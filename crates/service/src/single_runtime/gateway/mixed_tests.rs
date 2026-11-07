//! Actual Sail dynamic capture + existing authenticated mixed TCP/UDP.
//! Packet transport is isolated: this is not macOS OS-route/DNS acceptance.
use super::*;
use base64::Engine as _;
use ppvpn_core::capture_host::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
};
#[cfg(test)]
#[derive(Default)]
struct PacketHost;
#[cfg(test)]
#[async_trait]
impl CaptureHostApi for PacketHost {
    async fn prepare(
        &self,
        _: &ForwarderIdentity,
        _: u64,
        _: &ppvpn_core_wire::CaptureSpec,
        _: &CaptureNetworkSettings,
    ) -> CaptureHostResult<Arc<dyn CaptureAttachment>> {
        Ok(Arc::new(PacketResource {
            flow: Mutex::new(None),
        }))
    }
}
#[cfg(test)]
struct PacketResource {
    flow: Mutex<Option<CapturePacketFlow>>,
}
#[cfg(test)]
#[async_trait]
impl CaptureAttachment for PacketResource {
    fn interface_name(&self) -> &str {
        ""
    }
    fn transport(&self) -> CaptureTransport {
        CaptureTransport::Packets { capacity: 4 }
    }
    fn publish_packets(&self, flow: CapturePacketFlow) -> CaptureHostResult<()> {
        *self.flow.lock() = Some(flow);
        Ok(())
    }
    async fn commit_attach(&self) -> CaptureHostResult<()> {
        Ok(())
    }
    async fn rollback_attach(&self) -> CaptureHostResult<()> {
        self.flow.lock().take();
        Ok(())
    }
    async fn commit_detach(&self) -> CaptureHostResult<()> {
        Ok(())
    }
    async fn rollback_detach(&self) -> CaptureHostResult<()> {
        Ok(())
    }
}
// The upstream fixture is a separate Go/sing-box loopback server. It has no
// ppvpn-core or Sail runtime and counts actual decoded SS TCP and sink accepts.
struct NodeFixture {
    child: tokio::process::Child,
    ss_port: u16,
    sink: std::net::SocketAddr,
    stats: std::net::SocketAddr,
}
impl NodeFixture {
    async fn start(required: bool) -> Option<Self> {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let path = match std::env::var_os("PPVPN_CAPTURE_NODE") {
            Some(path) => std::path::PathBuf::from(path).canonicalize().unwrap(),
            None if !required => return None,
            None => panic!("PPVPN_CAPTURE_NODE must name the real SS 2022 fixture"),
        };
        let mut child = {
            let _gate = ppvpn_core::capture_host::fd_spawn_guard();
            tokio::process::Command::new(path)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::inherit())
                .kill_on_drop(true)
                .spawn()
                .unwrap()
        };
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let line = tokio::time::timeout(Duration::from_secs(20), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .expect("SS fixture readiness");
        let info: Value = serde_json::from_str(&line).unwrap();
        Some(Self {
            child,
            ss_port: info["ss_port"].as_u64().unwrap().try_into().unwrap(),
            sink: info["sink"].as_str().unwrap().parse().unwrap(),
            stats: info["stats"].as_str().unwrap().parse().unwrap(),
        })
    }
    async fn counters(&self) -> Value {
        let mut stream = TcpStream::connect(self.stats).await.unwrap();
        stream
            .write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        assert!(header(&mut stream).await.starts_with(b"HTTP/1.0 200"));
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }
    async fn stop(mut self) {
        self.child.stdin.take();
        if tokio::time::timeout(Duration::from_secs(5), self.child.wait())
            .await
            .is_err()
        {
            self.child.kill().await.unwrap();
        }
    }
}
async fn header(stream: &mut TcpStream) -> Vec<u8> {
    let mut data = vec![];
    while !data.ends_with(b"\r\n\r\n") {
        data.push(stream.read_u8().await.unwrap());
        assert!(data.len() < 8192);
    }
    data
}
async fn socks_auth(address: &str, user: &str, password: &str) -> TcpStream {
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream.write_all(&[5, 1, 2]).await.unwrap();
    let mut reply = [0; 2];
    stream.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply, [5, 2]);
    let mut auth = vec![1, user.len() as u8];
    auth.extend_from_slice(user.as_bytes());
    auth.push(password.len() as u8);
    auth.extend_from_slice(password.as_bytes());
    stream.write_all(&auth).await.unwrap();
    stream.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply, [1, 0]);
    stream
}
async fn socks_reply(stream: &mut TcpStream) -> std::net::SocketAddr {
    let mut reply = [0; 4];
    stream.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply[0..2], [5, 0]);
    match reply[3] {
        1 => {
            let mut address = [0; 6];
            stream.read_exact(&mut address).await.unwrap();
            std::net::SocketAddr::from((
                [address[0], address[1], address[2], address[3]],
                u16::from_be_bytes([address[4], address[5]]),
            ))
        }
        _ => panic!("expected loopback IPv4 relay"),
    }
}
// Native-only probes originate as ordinary OS sockets: unlike the held mixed
// streams they must enter the utun before reaching core DNS and physical egress.
async fn native_ingress_probes(sequence: u16) {
    let http=tokio::time::timeout(Duration::from_secs(10),async {
        let mut tcp=TcpStream::connect("1.1.1.1:80").await?;
        tcp.write_all(b"GET / HTTP/1.0\r\nHost: one.one.one.one\r\n\r\n").await?;
        let mut response=Vec::new();
        while !response.ends_with(b"\r\n\r\n") {
            response.push(tcp.read_u8().await?);
            if response.len()>=8192 { return Err(std::io::Error::other("HTTP_HEADER_LIMIT")); }
        }
        Ok::<_,std::io::Error>(response.starts_with(b"HTTP/1."))
    }).await;
    let http_ok=matches!(http,Ok(Ok(true)));
    eprintln!("native-ingress physical-http={http_ok} outcome={:?}",http.as_ref().map(|r|r.as_ref().map(|_|()).map_err(|e|(e.kind(),e.raw_os_error()))));
    let socket = UdpSocket::bind("0.0.0.0:0").await.unwrap();
    let mut query = sequence.to_be_bytes().to_vec();
    query.extend_from_slice(&[1, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
    query.extend_from_slice(b"\x07example\x03com\0\0\x01\0\x01");
    socket.send_to(&query, "10.60.159.90:53").await.unwrap();
    let mut answer = [0u8; 2048];
    let (n, peer) = tokio::time::timeout(Duration::from_secs(10), socket.recv_from(&mut answer))
        .await
        .unwrap()
        .unwrap();
    assert!(n > 12);
    assert_eq!(
        peer,
        "10.60.159.90:53".parse::<std::net::SocketAddr>().unwrap()
    );
    assert_eq!(&answer[..2], &sequence.to_be_bytes());
    eprintln!("native-ingress dns-rcode={} answers={}",answer[3]&15,u16::from_be_bytes([answer[6],answer[7]]));
    assert_eq!(answer[3] & 15, 0);
    assert!(u16::from_be_bytes([answer[6], answer[7]]) > 0);
    assert!(http_ok,"native HTTP probe failed");
}
async fn echo(stream: &mut TcpStream, sequence: u32) {
    let bytes = sequence.to_be_bytes();
    stream.write_all(&bytes).await.unwrap();
    let mut reply = [0; 4];
    stream.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply, bytes);
}
#[cfg(test)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authenticated_http_socks_tcp_udp_survive_native_capture_transactions_and_ui_detach() {
    exercise(Arc::new(PacketHost), false).await;
}
pub(super) async fn exercise(host: Arc<dyn CaptureHostApi>, native: bool) {
    tokio::time::timeout(Duration::from_secs(180),async {
        let node_fixture=NodeFixture::start(native).await;
        let dir=std::env::temp_dir().join(format!("ppvpn-mixed-{}",uuid::Uuid::new_v4()));
        let tcp=TcpListener::bind("127.0.0.1:0").await.unwrap();let target=tcp.local_addr().unwrap();
        let tcp_task=tokio::spawn(async move {loop {let(mut socket,_)=tcp.accept().await.unwrap();tokio::spawn(async move {let(mut read,mut write)=socket.split();let _=tokio::io::copy(&mut read,&mut write).await;});}});
        let udp=UdpSocket::bind("127.0.0.1:0").await.unwrap();let udp_target=udp.local_addr().unwrap();
        let udp_task=tokio::spawn(async move {let mut packet=[0;2048];loop {let(n,peer)=udp.recv_from(&mut packet).await.unwrap();udp.send_to(&packet[..n],peer).await.unwrap();}});
        let backend=EngineForwarder::new(ppvpn_core::EngineConfig::new(ppvpn_core::Role::Shared,ppvpn_core::Platform::Macos,dir.clone()).with_capture_host(CaptureHost::new(host)).with_local_proxy(ppvpn_core::LocalProxyConfig::new().with_preferred_port(0)).with_log(ppvpn_core::LogConfig::new(ppvpn_core::LogLevel::Debug,ppvpn_core::LogSink::Channel))).await.unwrap();
        // Only fixed-fixture DNS diagnostics leave the QA process. Never emit
        // arbitrary core logs, profile credentials, or IPC/session identifiers.
        let mut lines=backend.logs();
        let dns_logging=tokio::spawn(async move {
            while let Some(line)=lines.recv().await {
                if native && (line.contains("local dns servers") || line.contains("server=dns-local") || line.contains("server=\"dns-local\"")) { eprintln!("native-dns {line}"); }
            }
        });
        let mut profile:Value=serde_json::from_str(include_str!("../../../../account/tests/fixtures/proxy-profile.json")).unwrap();
        profile["nodes"].as_array_mut().unwrap().truncate(1);
        profile["nodes"][0]["ingresses"].as_array_mut().unwrap().truncate(1);
        profile["routing"]=json!({"rule_sets":[],"rules":[],"final":{"type":"direct"}});
        for node in profile["nodes"].as_array_mut().unwrap() {for ingress in node["ingresses"].as_array_mut().unwrap() {ingress["endpoint"]=json!({"domain":"127.0.0.1","port":9});}}
        if let Some(node)=&node_fixture {
            let ingress=&mut profile["nodes"][0]["ingresses"][0];
            ingress["endpoint"]=json!({"domain":"127.0.0.1","port":node.ss_port});
            ingress["credentials"]=json!({"shadowsocks":{"method":"2022-blake3-aes-128-gcm","user_key":"AAAAAAAAAAAAAAAAAAAAAA=="}});
        }
        let node_id=profile["nodes"][0]["id"].as_str().unwrap().to_owned();
        backend.call("/v1/apply-profile",json!({"profile":profile,"routing_mode":"rules"})).await.unwrap();
        // Exercise real physical auto-binding with loopback destinations. A
        // blanket IP_BOUND_IF host callback breaks core's own DNS-local loopback.
        let physical=crate::single_runtime::native::physical().ok()
            .filter(|s|s.interface.is_some()).unwrap_or_else(||ppvpn_core::NetworkState::new("lo0",ppvpn_core::NetworkKind::Other).with_addresses(vec!["127.0.0.1/8".into()]));
        backend.network(physical).unwrap();
        backend.call("/v1/start",json!({})).await.unwrap();
        let credential=backend.call("/v1/get-local-proxy-credential",json!({"kind":"routed"})).await.unwrap();
        let user=credential["username"].as_str().unwrap();let password=credential["password"].as_str().unwrap();let proxy=format!("127.0.0.1:{}",credential["port"].as_u64().unwrap());
        let identity=backend.identity();let gateway=Gateway::new(backend.clone(),unsafe { libc::geteuid() }).unwrap();
        let mut denied=TcpStream::connect(&proxy).await.unwrap();denied.write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes()).await.unwrap();assert!(header(&mut denied).await.starts_with(b"HTTP/1.1 407"));drop(denied);
        let mut http=TcpStream::connect(&proxy).await.unwrap();let basic=base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"));
        http.write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: Basic {basic}\r\n\r\n").as_bytes()).await.unwrap();assert!(header(&mut http).await.starts_with(b"HTTP/1.1 200"));
        let mut socks=socks_auth(&proxy,user,password).await;let mut connect=vec![5,1,0,1,127,0,0,1];connect.extend_from_slice(&target.port().to_be_bytes());socks.write_all(&connect).await.unwrap();let _=socks_reply(&mut socks).await;
        let mut association=socks_auth(&proxy,user,password).await;association.write_all(&[5,3,0,1,0,0,0,0,0,0]).await.unwrap();let mut relay=socks_reply(&mut association).await;if relay.ip().is_unspecified() {relay.set_ip("127.0.0.1".parse().unwrap());}
        let udp_client=UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut per_node=None;
        if let Some(node)=&node_fixture {
            let node_credential=backend.call("/v1/get-local-proxy-credential",json!({"node_id":node_id})).await.unwrap();
            assert_ne!(node_credential["username"],credential["username"]);
            assert_eq!(node_credential["port"],credential["port"]);
            let mut stream=socks_auth(&proxy,node_credential["username"].as_str().unwrap(),node_credential["password"].as_str().unwrap()).await;
            let mut request=vec![5,1,0,1,127,0,0,1];request.extend_from_slice(&node.sink.port().to_be_bytes());
            stream.write_all(&request).await.unwrap();let _=socks_reply(&mut stream).await;
            echo(&mut stream,1000).await;
            let counts=node.counters().await;
            assert_eq!(counts["upstream_connections"],1);assert_eq!(counts["sink_accepts"],1);
            per_node=Some((stream,node_credential));
        }
        let mut epoch=0;
        for sequence in 0..20 {
            let attached=backend.call("/v1/attach-capture",json!({"protocol_version":1,"expected_epoch":epoch,"spec":{"resource":{"kind":"host_lease","lease_id":uuid::Uuid::new_v4().to_string()},"ipv6":true}})).await.unwrap();
            assert_eq!(attached["phase"],"attached");
            if native {
                native_ingress_probes(sequence as u16).await;
                assert_eq!(ppvpn_core::capture_host::forwarding_runtime_ids().len(),1);
                println!("{}",json!({"probe":"attached","cycle":sequence,"owner":identity,"runtime_ids":ppvpn_core::capture_host::forwarding_runtime_ids(),"broker_registry_verified_empty":true}));
            }
            echo(&mut http,sequence).await;echo(&mut socks,sequence).await;
            if let Some((stream,_))=&mut per_node { echo(stream,sequence+2000).await; }
            let mut packet=vec![0,0,0,1,127,0,0,1];packet.extend_from_slice(&udp_target.port().to_be_bytes());packet.extend_from_slice(&sequence.to_be_bytes());udp_client.send_to(&packet,relay).await.unwrap();let mut reply=[0;2048];let(n,_)=udp_client.recv_from(&mut reply).await.unwrap();assert_eq!(&reply[n-4..n],&sequence.to_be_bytes());
            let detached=backend.call("/v1/detach-capture",json!({"protocol_version":1,"expected_epoch":attached["epoch"],"lease":attached["lease"]})).await.unwrap();
            assert_eq!(detached["phase"],"detached");epoch=detached["epoch"].as_u64().unwrap();
            if native { println!("{}",json!({"probe":"detached","cycle":sequence,"owner":identity,"runtime_ids":ppvpn_core::capture_host::forwarding_runtime_ids()})); }
            echo(&mut http,sequence+100).await;echo(&mut socks,sequence+100).await;
            if let Some((stream,node_credential))=&mut per_node {
                echo(stream,sequence+3000).await;
                let after=backend.call("/v1/get-local-proxy-credential",json!({"node_id":node_id})).await.unwrap();assert_eq!(*node_credential,after);
                let counts=node_fixture.as_ref().unwrap().counters().await;assert_eq!(counts["upstream_connections"],1);assert_eq!(counts["sink_accepts"],1);
            }
            let after=backend.call("/v1/get-local-proxy-credential",json!({"kind":"routed"})).await.unwrap();assert!(credential==after);assert_eq!(identity,backend.identity());
        }
        drop(gateway);echo(&mut http,99).await;echo(&mut socks,99).await;
        if let Some((stream,_))=&mut per_node { echo(stream,9999).await; }
        let per_node_counts=if let Some(node)=&node_fixture { Some(node.counters().await) } else { None };
        drop(per_node);
        drop((http,socks,association,udp_client));backend.shutdown().await.unwrap();dns_logging.abort();tcp_task.abort();udp_task.abort();let _=std::fs::remove_dir_all(dir);
        if native { assert!(ppvpn_core::capture_host::forwarding_runtime_ids().is_empty()); println!("{}",json!({"probe":"complete","cycles":20,"http_connect":true,"socks_tcp":true,"socks_udp":true,"per_node_tcp":per_node_counts.is_some(),"per_node_upstream_connections":per_node_counts.as_ref().map(|c|c["upstream_connections"].clone()),"per_node_sink_accepts":per_node_counts.as_ref().map(|c|c["sink_accepts"].clone()),"runtime_ids":[]})); }
        if let Some(node)=node_fixture { node.stop().await; }
    }).await.expect("mixed traffic fixture must be bounded");
}
