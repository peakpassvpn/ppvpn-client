//! #45 dns-local group C: what a query gets. The resolvers are fakes on
//! loopback; the servers the cache hands out are documentation addresses
//! (loopback ones are never usable), mapped onto the fakes by [`MapDial`].

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use hickory_proto::op::{Message, MessageType, OpCode, Query, ResponseCode};
use hickory_proto::rr::rdata::A;
use hickory_proto::rr::{Name, RData, Record, RecordType};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};

use super::*;

/// Dials the fake behind a documentation address.
struct MapDial(HashMap<SocketAddr, SocketAddr>);

#[async_trait]
impl Dial for MapDial {
    async fn udp(&self, server: SocketAddr) -> io::Result<UdpSocket> {
        let socket = UdpSocket::bind("127.0.0.1:0").await?;
        socket.connect(self.0[&server]).await?;
        Ok(socket)
    }
    async fn tcp(&self, server: SocketAddr) -> io::Result<TcpStream> {
        TcpStream::connect(self.0[&server]).await
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Fake {
    /// Answers with 192.0.2.<n>.
    Answers(u8),
    /// Never answers.
    Silent,
    /// Truncated over UDP, the full answer over TCP.
    Truncates(u8),
    /// A reply with another id first, then the answer.
    WrongIdFirst(u8),
}

fn answer_for(query: &Message, last: u8, truncated: bool) -> Vec<u8> {
    let mut answer = Message::new(query.id, MessageType::Response, OpCode::Query);
    answer.metadata.truncation = truncated;
    answer.add_query(query.queries[0].clone());
    if !truncated {
        let rdata = RData::A(A::new(192, 0, 2, last));
        answer.add_answer(Record::from_rdata(query.queries[0].name.clone(), 60, rdata));
    }
    answer.to_vec().unwrap()
}

/// Starts a fake on loopback (UDP and TCP on the same port); counts queries.
async fn start(kind: Fake, queries: Arc<AtomicU32>) -> SocketAddr {
    // A free port for both: Windows excludes port ranges per protocol (its
    // runners' Hyper-V ranges), so a UDP port may be forbidden for TCP
    // (10013); try another until one takes both.
    let (udp, tcp) = 'bind: {
        for _ in 0..20 {
            let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
            if let Ok(udp) = UdpSocket::bind(tcp.local_addr().unwrap()).await {
                break 'bind (udp, tcp);
            }
        }
        panic!("no loopback port free for both UDP and TCP");
    };
    let addr = udp.local_addr().unwrap();
    let counted = queries.clone();
    tokio::spawn(async move {
        let mut buffer = vec![0u8; 65535];
        while let Ok((n, peer)) = udp.recv_from(&mut buffer).await {
            counted.fetch_add(1, Ordering::SeqCst);
            let query = Message::from_vec(&buffer[..n]).unwrap();
            match kind {
                Fake::Silent => {}
                Fake::Answers(last) => {
                    udp.send_to(&answer_for(&query, last, false), peer)
                        .await
                        .unwrap();
                }
                Fake::Truncates(_) => {
                    udp.send_to(&answer_for(&query, 0, true), peer)
                        .await
                        .unwrap();
                }
                Fake::WrongIdFirst(last) => {
                    let mut other = query.clone();
                    other.metadata.id = query.id.wrapping_add(1);
                    udp.send_to(&answer_for(&other, 99, false), peer)
                        .await
                        .unwrap();
                    udp.send_to(&answer_for(&query, last, false), peer)
                        .await
                        .unwrap();
                }
            }
        }
    });
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = tcp.accept().await {
            queries.fetch_add(1, Ordering::SeqCst);
            let mut length = [0u8; 2];
            stream.read_exact(&mut length).await.unwrap();
            let mut buffer = vec![0u8; u16::from_be_bytes(length) as usize];
            stream.read_exact(&mut buffer).await.unwrap();
            let query = Message::from_vec(&buffer).unwrap();
            let Fake::Truncates(last) = kind else {
                continue;
            };
            let reply = answer_for(&query, last, false);
            stream
                .write_all(&(reply.len() as u16).to_be_bytes())
                .await
                .unwrap();
            stream.write_all(&reply).await.unwrap();
        }
    });
    addr
}

struct Rig {
    dns: LocalDns,
    reads: Arc<AtomicU32>,
    queries: Vec<Arc<AtomicU32>>,
}

/// dns-local on en0 with one fake per entry, as servers 192.0.2.1, .2, ...
async fn new_rig(fakes: &[Fake], hosts: &str) -> Rig {
    let mut map = HashMap::new();
    let mut servers = Vec::new();
    let mut queries = Vec::new();
    for (i, kind) in fakes.iter().enumerate() {
        let count = Arc::new(AtomicU32::new(0));
        let doc: SocketAddr = format!("192.0.2.{}:53", i + 1).parse().unwrap();
        map.insert(doc, start(*kind, count.clone()).await);
        servers.push(Server::new(doc.ip(), 53));
        queries.push(count);
    }
    let reads = Arc::new(AtomicU32::new(0));
    let counted = reads.clone();
    let started = Instant::now();
    let cache = Cache::new(
        move |_: &Interface| {
            counted.fetch_add(1, Ordering::SeqCst);
            Discovered::read("fake", servers.clone())
        },
        || {
            Some(Interface {
                index: 6,
                name: "en0".into(),
            })
        },
        servers::tunnel_prefixes(),
        move || started.elapsed(),
        |_: &Change| {},
    );
    let dns = LocalDns::new(
        Arc::new(cache),
        Arc::new(Hosts::parse(hosts)),
        Arc::new(MapDial(map)),
    );
    Rig {
        dns,
        reads,
        queries,
    }
}

fn query(name: &str, kind: RecordType) -> Message {
    let mut query = Message::new(0x4b4b, MessageType::Query, OpCode::Query);
    query.metadata.recursion_desired = true;
    query.add_query(Query::query(Name::from_ascii(name).unwrap(), kind));
    query
}

fn addresses(answer: &Message) -> Vec<String> {
    answer.answers.iter().map(|r| r.data.to_string()).collect()
}

// C1: no server to ask, or no default interface: SERVFAIL at once.
#[tokio::test]
async fn without_servers_servfail_at_once() {
    let rig = new_rig(&[], "").await;
    let started = Instant::now();
    let answer = rig
        .dns
        .exchange(&query("www.example.", RecordType::A))
        .await;
    assert!(
        started.elapsed() < Duration::from_millis(100),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(answer.response_code, ResponseCode::ServFail);
    assert_eq!((answer.id, answer.queries.len()), (0x4b4b, 1));

    let offline = Cache::new(
        |_: &Interface| unreachable!("no read while offline"),
        || None,
        Vec::new(),
        || Duration::ZERO,
        |_: &Change| {},
    );
    let dns = LocalDns::new(
        Arc::new(offline),
        Arc::new(Hosts::default()),
        Arc::new(PlainDial),
    );
    let answer = dns.exchange(&query("www.example.", RecordType::A)).await;
    assert_eq!(answer.response_code, ResponseCode::ServFail);
}

// C2: the servers in order, each within SERVER_TIMEOUT. B4: when all fail,
// SERVFAIL and the next query reads the servers again.
#[tokio::test]
async fn next_server_after_a_silent_one() {
    let rig = new_rig(&[Fake::Silent, Fake::Answers(2)], "").await;
    let started = Instant::now();
    let answer = rig
        .dns
        .exchange(&query("www.example.", RecordType::A))
        .await;
    let took = started.elapsed();
    assert_eq!(addresses(&answer), ["192.0.2.2"]);
    assert!(
        took >= exchange::SERVER_TIMEOUT
            && took < exchange::SERVER_TIMEOUT + Duration::from_secs(1),
        "{took:?}"
    );
    assert_eq!(rig.queries[0].load(Ordering::SeqCst), 1);
    assert_eq!(rig.queries[1].load(Ordering::SeqCst), 1);

    let silent = new_rig(&[Fake::Silent], "").await;
    let answer = silent
        .dns
        .exchange(&query("www.example.", RecordType::A))
        .await;
    assert_eq!(answer.response_code, ResponseCode::ServFail);
    assert_eq!(silent.reads.load(Ordering::SeqCst), 1);
    // Marked suspect: read again once RETRY_INTERVAL has passed (the
    // exchange above took SERVER_TIMEOUT, longer than that).
    silent
        .dns
        .exchange(&query("www.example.", RecordType::A))
        .await;
    assert_eq!(silent.reads.load(Ordering::SeqCst), 2);
}

// C3: a truncated UDP answer is asked again over TCP, same server.
#[tokio::test]
async fn truncated_answer_retries_over_tcp() {
    let rig = new_rig(&[Fake::Truncates(3)], "").await;
    let answer = rig
        .dns
        .exchange(&query("big.example.", RecordType::A))
        .await;
    assert!(!answer.truncation);
    assert_eq!(addresses(&answer), ["192.0.2.3"]);
    assert_eq!(rig.queries[0].load(Ordering::SeqCst), 2, "UDP, then TCP");
}

// C4: a reply with another id is dropped; the real answer is waited for.
#[tokio::test]
async fn reply_with_another_id_is_dropped() {
    let rig = new_rig(&[Fake::WrongIdFirst(4)], "").await;
    let answer = rig
        .dns
        .exchange(&query("www.example.", RecordType::A))
        .await;
    assert_eq!(
        (answer.id, addresses(&answer)),
        (0x4b4b, vec!["192.0.2.4".to_string()])
    );
}

// C5: the answer comes with the server that gave it (the `msg=dns
// upstream=` debug line).
#[tokio::test]
async fn answer_names_its_upstream() {
    let rig = new_rig(&[Fake::Silent, Fake::Answers(5)], "").await;
    let dial: Arc<dyn Dial> = rig.dns.dial.clone();
    let servers: Vec<SocketAddr> = vec![
        "192.0.2.1:53".parse().unwrap(),
        "192.0.2.2:53".parse().unwrap(),
    ];
    let (answer, upstream) =
        exchange::exchange(&dial, &servers, &query("www.example.", RecordType::A))
            .await
            .unwrap();
    assert_eq!(
        (upstream.to_string(), addresses(&answer)),
        ("192.0.2.2:53".to_string(), vec!["192.0.2.5".to_string()])
    );
}

// C6: names in the hosts file are answered from it, without a query or a read.
#[tokio::test]
async fn hosts_file_names_are_answered_locally() {
    let rig = new_rig(
        &[Fake::Answers(6)],
        "192.0.2.77 printer.lan\nfe80::77 printer.lan\n",
    )
    .await;
    let answer = rig
        .dns
        .exchange(&query("Printer.LAN.", RecordType::A))
        .await;
    assert_eq!(
        (answer.response_code, addresses(&answer)),
        (ResponseCode::NoError, vec!["192.0.2.77".to_string()])
    );
    let answer = rig
        .dns
        .exchange(&query("printer.lan.", RecordType::AAAA))
        .await;
    assert_eq!(addresses(&answer), ["fe80::77"]);
    let answer = rig
        .dns
        .exchange(&query("printer.lan.", RecordType::MX))
        .await;
    assert_eq!(
        addresses(&answer),
        ["192.0.2.6"],
        "not A or AAAA: asked upstream"
    );
    assert_eq!(rig.reads.load(Ordering::SeqCst), 1);
    assert_eq!(rig.queries[0].load(Ordering::SeqCst), 1);
}

#[test]
fn servfail_keeps_the_question() {
    let q = query("www.example.", RecordType::AAAA);
    let answer = exchange::server_failure(&q);
    assert_eq!(answer.response_code, ResponseCode::ServFail);
    assert!(exchange::answers(&answer, &q));
    assert!(answer.recursion_desired && answer.recursion_available);
}

#[test]
fn error_texts() {
    assert_eq!(
        LocalDnsError::NoInterface.to_string(),
        "no default interface"
    );
    let none = LocalDnsError::NoServers {
        interface: "en0".into(),
        source: "scutil-global".into(),
    };
    assert_eq!(none.to_string(), "no DNS servers on en0 (scutil-global)");
}
