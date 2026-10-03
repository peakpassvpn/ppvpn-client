//! Asking the resolvers (Go: internal/localdns/localdns.go `Exchange`).

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use hickory_proto::op::{Message, MessageType, ResponseCode};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};

use crate::runtime::{AsyncReadWrite, Datagram, Runtime, Target};

/// How long one server has for one exchange.
pub const SERVER_TIMEOUT: Duration = Duration::from_secs(2);

/// Opens a datagram or stream to a resolver. [`RuntimeDial`] goes through
/// sail's direct outbound, whose dialer is bound to the default interface,
/// so that a query to a physical resolver never enters the TUN (#45
/// dns-local case D1); [`PlainDial`] does not bind (tests).
#[async_trait]
pub(crate) trait Dial: Send + Sync {
    async fn udp(&self, server: SocketAddr) -> io::Result<Box<dyn Datagram>>;
    async fn tcp(&self, server: SocketAddr) -> io::Result<Box<dyn AsyncReadWrite>>;
}

/// Through one outbound of the running sail (the direct one): sail binds the
/// socket to the default interface and follows it across changes. A
/// link-local server's zone is its `scope_id` (the interface index), which
/// sail passes to the kernel as it is.
pub(crate) struct RuntimeDial {
    pub runtime: Arc<dyn Runtime>,
    pub outbound: String,
}

fn io_error(e: crate::runtime::RuntimeError) -> io::Error {
    io::Error::other(e.to_string())
}

#[async_trait]
impl Dial for RuntimeDial {
    async fn udp(&self, server: SocketAddr) -> io::Result<Box<dyn Datagram>> {
        self.runtime
            .dial_udp(&self.outbound, Target::Addr(server), SERVER_TIMEOUT)
            .await
            .map_err(io_error)
    }
    async fn tcp(&self, server: SocketAddr) -> io::Result<Box<dyn AsyncReadWrite>> {
        self.runtime
            .dial_tcp(&self.outbound, Target::Addr(server), SERVER_TIMEOUT)
            .await
            .map_err(io_error)
    }
}

/// Unbound sockets.
pub(crate) struct PlainDial;

struct Connected(UdpSocket);

#[async_trait]
impl Datagram for Connected {
    async fn send(&self, data: &[u8]) -> io::Result<()> {
        self.0.send(data).await.map(|_| ())
    }
    async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.recv(buf).await
    }
}

#[async_trait]
impl Dial for PlainDial {
    async fn udp(&self, server: SocketAddr) -> io::Result<Box<dyn Datagram>> {
        let bind: SocketAddr = if server.is_ipv4() {
            "0.0.0.0:0".parse().unwrap()
        } else {
            "[::]:0".parse().unwrap()
        };
        let socket = UdpSocket::bind(bind).await?;
        socket.connect(server).await?;
        Ok(Box::new(Connected(socket)))
    }
    async fn tcp(&self, server: SocketAddr) -> io::Result<Box<dyn AsyncReadWrite>> {
        Ok(Box::new(TcpStream::connect(server).await?))
    }
}

/// Asks `servers` in order, each within [`SERVER_TIMEOUT`]: UDP, and TCP
/// again when the answer is truncated. The first answer is returned with
/// the server that gave it; otherwise the errors, one per server.
pub(crate) async fn exchange(
    dial: &Arc<dyn Dial>,
    servers: &[SocketAddr],
    query: &Message,
) -> Result<(Message, SocketAddr), Vec<String>> {
    let packed = query.to_vec().map_err(|e| vec![format!("pack: {e}")])?;
    let mut errors = Vec::new();
    for server in servers {
        match tokio::time::timeout(
            SERVER_TIMEOUT,
            exchange_one(dial.as_ref(), *server, query, &packed),
        )
        .await
        {
            Ok(Ok(answer)) => return Ok((answer, *server)),
            Ok(Err(e)) => errors.push(format!("{server}: {e}")),
            Err(_) => errors.push(format!("{server}: timed out")),
        }
    }
    Err(errors)
}

async fn exchange_one(
    dial: &dyn Dial,
    server: SocketAddr,
    query: &Message,
    packed: &[u8],
) -> io::Result<Message> {
    let answer = exchange_udp(dial, server, query, packed).await?;
    if !answer.truncation {
        return Ok(answer);
    }
    exchange_tcp(dial, server, query, packed).await
}

async fn exchange_udp(
    dial: &dyn Dial,
    server: SocketAddr,
    query: &Message,
    packed: &[u8],
) -> io::Result<Message> {
    let socket = dial.udp(server).await?;
    socket.send(packed).await?;
    let mut buffer = vec![0u8; 65535];
    loop {
        let n = socket.recv(&mut buffer).await?;
        // A stray or late datagram: keep waiting for ours.
        if let Ok(answer) = Message::from_vec(&buffer[..n]) {
            if answers(&answer, query) {
                return Ok(answer);
            }
        }
    }
}

async fn exchange_tcp(
    dial: &dyn Dial,
    server: SocketAddr,
    query: &Message,
    packed: &[u8],
) -> io::Result<Message> {
    let mut stream = dial.tcp(server).await?;
    let mut framed = Vec::with_capacity(2 + packed.len());
    framed.extend_from_slice(&(packed.len() as u16).to_be_bytes());
    framed.extend_from_slice(packed);
    stream.write_all(&framed).await?;
    let mut length = [0u8; 2];
    stream.read_exact(&mut length).await?;
    let mut buffer = vec![0u8; u16::from_be_bytes(length) as usize];
    stream.read_exact(&mut buffer).await?;
    let answer = Message::from_vec(&buffer)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    if !answers(&answer, query) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "answer does not match the query",
        ));
    }
    Ok(answer)
}

/// Whether `answer` is the reply to `query`: same id, a response, the same
/// questions (names compared without case: servers may echo 0x20 names).
pub fn answers(answer: &Message, query: &Message) -> bool {
    answer.id == query.id
        && answer.message_type == MessageType::Response
        && answer.queries.len() == query.queries.len()
        && answer.queries.iter().zip(&query.queries).all(|(a, q)| {
            a.query_type == q.query_type && a.query_class() == q.query_class() && a.name == q.name
        })
}

/// SERVFAIL for `query`. Sent when no server could answer (none read, or all
/// failed): an error would leave a hijacked query unanswered and the client
/// waiting for its own timeout (#45 dns-local case C1). Never cached.
pub fn server_failure(query: &Message) -> Message {
    let mut response = Message::error_msg(query.id, query.op_code, ResponseCode::ServFail);
    response.metadata.recursion_desired = query.recursion_desired;
    response.metadata.recursion_available = true;
    for q in &query.queries {
        response.add_query(q.clone());
    }
    response
}
