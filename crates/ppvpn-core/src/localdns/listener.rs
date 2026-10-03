//! dns-local served to sail on loopback: sail's dns-local server is a `tcp`
//! server at [`Listener::addr`] (the translation renders it), and every
//! query it sends here is answered by [`LocalDns`]: the default interface's
//! resolvers asked in order through sail's direct outbound, hosts-file
//! names answered at once, SERVFAIL at once when there is no resolver.
//! sail's own `local` server is not used: it lacks what #45's dns-local
//! cases need (the comparison is in #45).
//!
//! TCP and UDP on the same port of 127.0.0.1; only loopback peers are
//! answered. A change of network does not touch the listener or sail's
//! configuration: [`Listener::invalidate`] makes the next query read the
//! interface again.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use hickory_proto::op::Message;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::task::JoinHandle;

use super::LocalDns;

/// How long a TCP connection from sail may stay idle between queries.
const TCP_IDLE: Duration = Duration::from_secs(30);

pub(crate) struct Listener {
    addr: SocketAddr,
    dns: Arc<LocalDns>,
    tasks: Vec<JoinHandle<()>>,
}

impl Listener {
    /// Where sail's dns-local server points.
    pub(crate) fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The network changed: the next query reads the interface again.
    pub(crate) fn invalidate(&self) {
        self.dns.cache().invalidate();
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Binds TCP and UDP on one free port of 127.0.0.1 and serves `dns` there
/// until the Listener is dropped.
pub(crate) async fn start(dns: Arc<LocalDns>) -> io::Result<Listener> {
    let (tcp, udp) = bind().await?;
    let addr = tcp.local_addr()?;
    let udp = Arc::new(udp);
    let mut tasks = Vec::new();
    let d = dns.clone();
    tasks.push(tokio::spawn(async move {
        let mut buffer = vec![0u8; 65535];
        while let Ok((n, peer)) = udp.recv_from(&mut buffer).await {
            if !peer.ip().is_loopback() {
                continue;
            }
            let Ok(query) = Message::from_vec(&buffer[..n]) else {
                continue;
            };
            let (udp, dns) = (udp.clone(), d.clone());
            tokio::spawn(async move {
                if let Ok(answer) = dns.exchange(&query).await.to_vec() {
                    let _ = udp.send_to(&answer, peer).await;
                }
            });
        }
    }));
    let d = dns.clone();
    tasks.push(tokio::spawn(async move {
        while let Ok((stream, peer)) = tcp.accept().await {
            if !peer.ip().is_loopback() {
                continue;
            }
            tokio::spawn(serve_stream(stream, d.clone()));
        }
    }));
    Ok(Listener { addr, dns, tasks })
}

/// One free port for both: Windows excludes port ranges per protocol, so a
/// TCP port may be forbidden for UDP; another is tried until one takes both.
async fn bind() -> io::Result<(TcpListener, UdpSocket)> {
    let loopback = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    let mut last = None;
    for _ in 0..20 {
        let tcp = TcpListener::bind(loopback).await?;
        match UdpSocket::bind(tcp.local_addr()?).await {
            Ok(udp) => return Ok((tcp, udp)),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("no loopback port free for both TCP and UDP")))
}

/// Length-prefixed queries on one connection, answered in order.
async fn serve_stream(mut stream: TcpStream, dns: Arc<LocalDns>) {
    loop {
        let mut length = [0u8; 2];
        match tokio::time::timeout(TCP_IDLE, stream.read_exact(&mut length)).await {
            Ok(Ok(_)) => {}
            _ => return,
        }
        let mut buffer = vec![0u8; u16::from_be_bytes(length) as usize];
        if stream.read_exact(&mut buffer).await.is_err() {
            return;
        }
        let Ok(query) = Message::from_vec(&buffer) else {
            return;
        };
        let Ok(answer) = dns.exchange(&query).await.to_vec() else {
            return;
        };
        let mut framed = Vec::with_capacity(2 + answer.len());
        framed.extend_from_slice(&(answer.len() as u16).to_be_bytes());
        framed.extend_from_slice(&answer);
        if stream.write_all(&framed).await.is_err() {
            return;
        }
    }
}
