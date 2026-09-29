// Package proxyinbound is the shared authenticated local proxy listener: one
// loopback port serving HTTP and SOCKS5, where the proxy username selects the
// node. It is a thin sing-box inbound whose only job beyond the stock mixed
// inbound is to verify the shared secret in constant time. Node selection is
// left to the router: the authenticated username is recorded as the
// connection's auth user and matched by `auth_user` route rules.
package proxyinbound

import (
	std_bufio "bufio"
	"bytes"
	"context"
	"crypto/subtle"
	"encoding/base64"
	"errors"
	"io"
	"net"
	"net/http"
	"strings"
	"time"

	"github.com/sagernet/sing-box/adapter"
	"github.com/sagernet/sing-box/adapter/inbound"
	"github.com/sagernet/sing-box/common/listener"
	"github.com/sagernet/sing-box/common/uot"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/log"
	"github.com/sagernet/sing-box/option"
	"github.com/sagernet/sing/common"
	"github.com/sagernet/sing/common/auth"
	"github.com/sagernet/sing/common/buf"
	"github.com/sagernet/sing/common/bufio"
	"github.com/sagernet/sing/common/canceler"
	E "github.com/sagernet/sing/common/exceptions"
	M "github.com/sagernet/sing/common/metadata"
	N "github.com/sagernet/sing/common/network"
	singhttp "github.com/sagernet/sing/protocol/http"
	"github.com/sagernet/sing/protocol/socks"
	"github.com/sagernet/sing/protocol/socks/socks4"
	"github.com/sagernet/sing/protocol/socks/socks5"
)

// Type is the sing-box inbound type registered by Register.
const Type = "ppvpn-local-proxy"

// maxHeaderBytes bounds the first HTTP request head that is inspected before
// the connection is handed to the HTTP proxy implementation.
const maxHeaderBytes = 16 << 10

// User is one accepted username/password pair.
type User struct {
	Username string `json:"username"`
	Password string `json:"password"`
}

// Options configures the listener. Users must not be empty: this inbound
// never serves unauthenticated clients.
type Options struct {
	option.ListenOptions
	Users []User `json:"users"`
}

// Register adds the inbound type to a sing-box inbound registry.
func Register(registry *inbound.Registry) {
	inbound.Register[Options](registry, Type, New)
}

var errAuthFailed = errors.New("local proxy authentication failed")

// verifier checks credentials. The password comparison is constant time and
// runs even for unknown usernames, so a response does not reveal which part
// was wrong or how much of the secret matched.
type verifier struct {
	users map[string][]byte
	dummy []byte
}

func newVerifier(users []User) (*verifier, error) {
	if len(users) == 0 {
		return nil, E.New("local proxy requires at least one user")
	}
	v := &verifier{users: make(map[string][]byte, len(users))}
	for _, user := range users {
		if user.Username == "" || user.Password == "" {
			return nil, E.New("local proxy user must have a username and password")
		}
		if _, dup := v.users[user.Username]; dup {
			return nil, E.New("duplicate local proxy user")
		}
		v.users[user.Username] = []byte(user.Password)
		v.dummy = []byte(user.Password)
	}
	return v, nil
}

func (v *verifier) verify(username, password string) bool {
	expected, known := v.users[username]
	if !known {
		expected = v.dummy
	}
	match := subtle.ConstantTimeCompare([]byte(password), expected) == 1
	return known && match
}

// authenticator mirrors the accepted users for the HTTP implementation, which
// re-checks every later request on a keep-alive connection. It is only
// reachable after this inbound verified the first request in constant time.
func (v *verifier) authenticator() *auth.Authenticator {
	users := make([]auth.User, 0, len(v.users))
	for name, password := range v.users {
		users = append(users, auth.User{Username: name, Password: string(password)})
	}
	return auth.NewAuthenticator(users)
}

type Inbound struct {
	inbound.Adapter
	router        adapter.ConnectionRouterEx
	logger        log.ContextLogger
	listener      *listener.Listener
	verifier      *verifier
	authenticator *auth.Authenticator
}

func New(ctx context.Context, router adapter.Router, logger log.ContextLogger, tag string, options Options) (adapter.Inbound, error) {
	v, err := newVerifier(options.Users)
	if err != nil {
		return nil, err
	}
	in := &Inbound{
		Adapter:       inbound.NewAdapter(Type, tag),
		router:        uot.NewRouter(router, logger),
		logger:        logger,
		verifier:      v,
		authenticator: v.authenticator(),
	}
	in.listener = listener.New(listener.Options{
		Context:           ctx,
		Logger:            logger,
		Network:           []string{N.NetworkTCP},
		Listen:            options.ListenOptions,
		ConnectionHandler: in,
	})
	return in, nil
}

func (h *Inbound) Start(stage adapter.StartStage) error {
	if stage != adapter.StartStateStart {
		return nil
	}
	return h.listener.Start()
}

func (h *Inbound) Close() error { return common.Close(h.listener) }

func (h *Inbound) NewConnectionEx(ctx context.Context, conn net.Conn, metadata adapter.InboundContext, onClose N.CloseHandlerFunc) {
	err := h.newConnection(ctx, conn, metadata, onClose)
	N.CloseOnHandshakeFailure(conn, onClose, err)
	if err != nil && !E.IsClosedOrCanceled(err) {
		// Never include credentials in the message: logs are disabled today,
		// but this keeps the credential policy independent of that.
		h.logger.DebugContext(ctx, "local proxy connection rejected")
	}
}

func (h *Inbound) newConnection(ctx context.Context, conn net.Conn, metadata adapter.InboundContext, onClose N.CloseHandlerFunc) error {
	reader := std_bufio.NewReaderSize(conn, maxHeaderBytes)
	header, err := reader.Peek(1)
	if err != nil {
		return E.Cause(err, "peek first byte")
	}
	handler := adapter.NewUpstreamHandlerEx(metadata, h.newUserConnection, h.newUserPacketConnection)
	switch header[0] {
	case socks4.Version:
		// SOCKS4 cannot carry a password.
		return errAuthFailed
	case socks5.Version:
		return h.handleSOCKS5(ctx, conn, reader, handler, metadata.Source, onClose)
	default:
		return h.handleHTTP(ctx, conn, reader, handler, metadata.Source, onClose)
	}
}

// handleHTTP verifies the first request's Proxy-Authorization in constant time
// without consuming it, then hands the connection to the HTTP proxy
// implementation, which re-reads the request and sets the auth user.
func (h *Inbound) handleHTTP(ctx context.Context, conn net.Conn, reader *std_bufio.Reader, handler N.TCPConnectionHandlerEx, source M.Socksaddr, onClose N.CloseHandlerFunc) error {
	head, err := peekRequestHead(reader)
	if err != nil {
		return err
	}
	request, err := http.ReadRequest(std_bufio.NewReader(bytes.NewReader(head)))
	if err != nil {
		return E.Cause(err, "read http request")
	}
	username, password, ok := basicProxyAuth(request.Header.Get("Proxy-Authorization"))
	if !ok || !h.verifier.verify(username, password) {
		response := "HTTP/1.1 407 Proxy Authentication Required\r\n" +
			"Proxy-Authenticate: Basic realm=\"ppvpn\", charset=\"UTF-8\"\r\n" +
			"Content-Length: 0\r\nConnection: close\r\n\r\n"
		if _, writeErr := io.WriteString(conn, response); writeErr != nil {
			return writeErr
		}
		return errAuthFailed
	}
	return singhttp.HandleConnectionEx(ctx, conn, reader, h.authenticator, handler, source, onClose)
}

func peekRequestHead(reader *std_bufio.Reader) ([]byte, error) {
	for n := 1; ; {
		peeked, err := reader.Peek(n)
		if err != nil {
			if errors.Is(err, std_bufio.ErrBufferFull) {
				return nil, E.New("http request head too large")
			}
			return nil, err
		}
		if end := headEnd(peeked); end > 0 {
			return peeked[:end], nil
		}
		if buffered := reader.Buffered(); buffered > n {
			n = buffered
		} else {
			n++
		}
	}
}

func headEnd(data []byte) int {
	if i := bytes.Index(data, []byte("\n\r\n")); i >= 0 {
		return i + 3
	}
	if i := bytes.Index(data, []byte("\n\n")); i >= 0 {
		return i + 2
	}
	return -1
}

func basicProxyAuth(header string) (string, string, bool) {
	const prefix = "Basic "
	// Case-sensitive like the HTTP implementation that re-checks later
	// requests, so both layers accept exactly the same headers.
	if !strings.HasPrefix(header, prefix) {
		return "", "", false
	}
	decoded, err := base64.StdEncoding.DecodeString(header[len(prefix):])
	if err != nil {
		return "", "", false
	}
	username, password, ok := strings.Cut(string(decoded), ":")
	return username, password, ok
}

// handleSOCKS5 runs the RFC 1928/1929 handshake with a constant-time secret
// check; only username/password authentication is offered.
func (h *Inbound) handleSOCKS5(ctx context.Context, conn net.Conn, reader *std_bufio.Reader, handler socks.HandlerEx, source M.Socksaddr, onClose N.CloseHandlerFunc) error {
	if _, err := reader.ReadByte(); err != nil {
		return err
	}
	authRequest, err := socks5.ReadAuthRequest0(reader)
	if err != nil {
		return err
	}
	if !common.Contains(authRequest.Methods, socks5.AuthTypeUsernamePassword) {
		if err = socks5.WriteAuthResponse(conn, socks5.AuthResponse{Method: socks5.AuthTypeNoAcceptedMethods}); err != nil {
			return err
		}
		return errAuthFailed
	}
	if err = socks5.WriteAuthResponse(conn, socks5.AuthResponse{Method: socks5.AuthTypeUsernamePassword}); err != nil {
		return err
	}
	credentials, err := socks5.ReadUsernamePasswordAuthRequest(reader)
	if err != nil {
		return err
	}
	response := socks5.UsernamePasswordAuthResponse{Status: socks5.UsernamePasswordStatusFailure}
	accepted := h.verifier.verify(credentials.Username, credentials.Password)
	if accepted {
		response.Status = socks5.UsernamePasswordStatusSuccess
	}
	if err = socks5.WriteUsernamePasswordAuthResponse(conn, response); err != nil {
		return err
	}
	if !accepted {
		return errAuthFailed
	}
	ctx = auth.ContextWithUser(ctx, credentials.Username)
	request, err := socks5.ReadRequest(reader)
	if err != nil {
		return err
	}
	switch request.Command {
	case socks5.CommandConnect:
		handler.NewConnectionEx(ctx, socks.NewLazyConn(cachedConn(conn, reader), socks5.Version), source, request.Destination, onClose)
		return nil
	case socks5.CommandUDPAssociate:
		return h.associateUDP(ctx, conn, handler, source, onClose)
	default:
		if err = socks5.WriteResponse(conn, socks5.Response{ReplyCode: socks5.ReplyCodeUnsupported}); err != nil {
			return err
		}
		return E.New("socks5: unsupported command")
	}
}

// associateUDP follows sing's SOCKS5 UDP ASSOCIATE handling: the relay socket
// is bound on the same loopback address as the control connection.
func (h *Inbound) associateUDP(ctx context.Context, conn net.Conn, handler socks.HandlerEx, source M.Socksaddr, onClose N.CloseHandlerFunc) error {
	local := M.AddrFromNet(conn.LocalAddr())
	udpConn, err := h.listener.ListenPacket(net.ListenConfig{}, ctx, M.NetworkFromNetAddr(N.NetworkUDP, local), M.SocksaddrFrom(local, 0).String())
	if err != nil {
		return E.Cause(err, "socks5: listen udp")
	}
	if err = socks5.WriteResponse(conn, socks5.Response{ReplyCode: socks5.ReplyCodeSuccess, Bind: M.SocksaddrFromNet(udpConn.LocalAddr()).Unwrap()}); err != nil {
		udpConn.Close()
		return E.Cause(err, "socks5: write response")
	}
	var packetConn N.PacketConn = socks.NewAssociatePacketConn(bufio.NewServerPacketConn(udpConn), M.Socksaddr{}, conn)
	udpConn.SetReadDeadline(time.Now().Add(C.UDPTimeout))
	first := buf.NewPacket()
	destination, err := packetConn.ReadPacket(first)
	if err != nil {
		first.Release()
		packetConn.Close()
		return E.Cause(err, "socks5: read first packet")
	}
	udpConn.SetReadDeadline(time.Time{})
	ctx, packetConn = canceler.NewPacketConn(ctx, packetConn, C.UDPTimeout)
	packetConn = bufio.NewCachedPacketConn(packetConn, first, destination)
	handler.NewPacketConnectionEx(ctx, packetConn, source, destination, onClose)
	return nil
}

// cachedConn keeps bytes a client pipelined after the SOCKS5 request.
func cachedConn(conn net.Conn, reader *std_bufio.Reader) net.Conn {
	if reader.Buffered() == 0 {
		return conn
	}
	buffer := buf.NewSize(reader.Buffered())
	if _, err := buffer.ReadFullFrom(reader, reader.Buffered()); err != nil {
		buffer.Release()
		return conn
	}
	return bufio.NewCachedConn(conn, buffer)
}

func (h *Inbound) newUserConnection(ctx context.Context, conn net.Conn, metadata adapter.InboundContext, onClose N.CloseHandlerFunc) {
	metadata.Inbound = h.Tag()
	metadata.InboundType = h.Type()
	metadata.User, _ = auth.UserFromContext[string](ctx)
	h.router.RouteConnectionEx(ctx, conn, metadata, onClose)
}

func (h *Inbound) newUserPacketConnection(ctx context.Context, conn N.PacketConn, metadata adapter.InboundContext, onClose N.CloseHandlerFunc) {
	metadata.Inbound = h.Tag()
	metadata.InboundType = h.Type()
	metadata.User, _ = auth.UserFromContext[string](ctx)
	h.router.RoutePacketConnectionEx(ctx, conn, metadata, onClose)
}
