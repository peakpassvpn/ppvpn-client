// Command ldnslab is the helper of the dns-local lab test (run.sh):
//
//	ldnslab serve -listen 10.201.0.1:53 -answer 192.0.2.1
//	    answers every A query with -answer (TTL 0), one line per query on
//	    stdout: "<unix ms> <name> from <source>".
//	ldnslab query -server 10.60.159.90:53 -name q1.lab.test [-timeout 3s]
//	    prints "ok <address> <ms>" or "fail <rcode or error> <ms>"; exits 0
//	    either way (run.sh judges the outcome).
//	ldnslab now
//	    prints the time in unix milliseconds (busybox date has no %N).
//	ldnslab apply-body profile.json
//	    prints the apply-profile request for the profile with every domain
//	    routed direct, no rules and a far expiry (no jq on lab hosts).
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"net"
	"os"
	"strings"
	"time"

	"github.com/miekg/dns"
)

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "usage: ldnslab serve|query|apply-body|now ...")
		os.Exit(2)
	}
	switch os.Args[1] {
	case "serve":
		serve(os.Args[2:])
	case "query":
		query(os.Args[2:])
	case "apply-body":
		applyBody(os.Args[2:])
	case "now":
		fmt.Println(time.Now().UnixMilli())
	default:
		fmt.Fprintln(os.Stderr, "usage: ldnslab serve|query|apply-body|now ...")
		os.Exit(2)
	}
}

func serve(args []string) {
	flags := flag.NewFlagSet("serve", flag.ExitOnError)
	listen := flags.String("listen", "", "UDP and TCP address")
	answer := flags.String("answer", "", "IPv4 address every A query gets")
	_ = flags.Parse(args)
	ip := net.ParseIP(*answer).To4()
	if *listen == "" || ip == nil {
		fmt.Fprintln(os.Stderr, "serve needs -listen and an IPv4 -answer")
		os.Exit(2)
	}
	handler := dns.HandlerFunc(func(w dns.ResponseWriter, request *dns.Msg) {
		response := new(dns.Msg)
		response.SetReply(request)
		if len(request.Question) > 0 {
			question := request.Question[0]
			fmt.Printf("%d %s from %s\n", time.Now().UnixMilli(), strings.ToLower(question.Name), w.RemoteAddr())
			if question.Qtype == dns.TypeA {
				response.Answer = []dns.RR{&dns.A{Hdr: dns.RR_Header{Name: question.Name, Rrtype: dns.TypeA, Class: dns.ClassINET}, A: ip}}
			}
		}
		_ = w.WriteMsg(response)
	})
	errs := make(chan error, 2)
	for _, network := range []string{"udp", "tcp"} {
		server := &dns.Server{Addr: *listen, Net: network, Handler: handler}
		go func() { errs <- server.ListenAndServe() }()
	}
	fmt.Fprintln(os.Stderr, <-errs)
	os.Exit(1)
}

func query(args []string) {
	flags := flag.NewFlagSet("query", flag.ExitOnError)
	server := flags.String("server", "", "server address")
	name := flags.String("name", "", "name to resolve (A)")
	timeout := flags.Duration("timeout", 3*time.Second, "query timeout")
	_ = flags.Parse(args)
	message := new(dns.Msg)
	message.SetQuestion(dns.Fqdn(*name), dns.TypeA)
	client := &dns.Client{Timeout: *timeout}
	started := time.Now()
	response, _, err := client.Exchange(message, *server)
	ms := time.Since(started).Milliseconds()
	switch {
	case err != nil:
		fmt.Printf("fail %s %d\n", strings.ReplaceAll(err.Error(), " ", "_"), ms)
	case response.Rcode != dns.RcodeSuccess:
		fmt.Printf("fail %s %d\n", dns.RcodeToString[response.Rcode], ms)
	case len(response.Answer) == 0:
		fmt.Printf("fail NOANSWER %d\n", ms)
	default:
		address := "?"
		if a, ok := response.Answer[0].(*dns.A); ok {
			address = a.A.String()
		}
		fmt.Printf("ok %s %d\n", address, ms)
	}
}

func applyBody(args []string) {
	if len(args) != 1 {
		fmt.Fprintln(os.Stderr, "usage: ldnslab apply-body profile.json")
		os.Exit(2)
	}
	data, err := os.ReadFile(args[0])
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	var profile map[string]any
	if err := json.Unmarshal(data, &profile); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	routing, _ := profile["routing"].(map[string]any)
	if routing == nil {
		routing = map[string]any{}
	}
	routing["rules"] = []any{}
	routing["final"] = map[string]any{"type": "direct"}
	profile["routing"] = routing
	profile["expires_at"] = "2099-01-01T00:00:00Z"
	_ = json.NewEncoder(os.Stdout).Encode(map[string]any{"profile": profile, "routing_mode": "rules"})
}
