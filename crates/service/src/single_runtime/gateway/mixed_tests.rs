//! Actual Sail dynamic capture + existing authenticated mixed TCP/UDP.
//! Packet transport is isolated: this is not macOS OS-route/DNS acceptance.
use super::*;
use base64::Engine as _;
use ppvpn_core::capture_host::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
};
#[derive(Default)]
struct PacketHost;
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
struct PacketResource {
    flow: Mutex<Option<CapturePacketFlow>>,
}
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
async fn echo(stream: &mut TcpStream, sequence: u32) {
    let bytes = sequence.to_be_bytes();
    stream.write_all(&bytes).await.unwrap();
    let mut reply = [0; 4];
    stream.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply, bytes);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authenticated_http_socks_tcp_udp_survive_native_capture_transactions_and_ui_detach() {
    tokio::time::timeout(Duration::from_secs(30),async {
        let dir=std::env::temp_dir().join(format!("ppvpn-mixed-{}",uuid::Uuid::new_v4()));
        let tcp=TcpListener::bind("127.0.0.1:0").await.unwrap();let target=tcp.local_addr().unwrap();
        let tcp_task=tokio::spawn(async move {loop {let(mut socket,_)=tcp.accept().await.unwrap();tokio::spawn(async move {let(mut read,mut write)=socket.split();let _=tokio::io::copy(&mut read,&mut write).await;});}});
        let udp=UdpSocket::bind("127.0.0.1:0").await.unwrap();let udp_target=udp.local_addr().unwrap();
        let udp_task=tokio::spawn(async move {let mut packet=[0;2048];loop {let(n,peer)=udp.recv_from(&mut packet).await.unwrap();udp.send_to(&packet[..n],peer).await.unwrap();}});
        let backend=EngineForwarder::new(ppvpn_core::EngineConfig::new(ppvpn_core::Role::Shared,ppvpn_core::Platform::Macos,dir.clone()).with_capture_host(CaptureHost::new(Arc::new(PacketHost))).with_local_proxy(ppvpn_core::LocalProxyConfig::new().with_preferred_port(0))).await.unwrap();
        let mut profile:Value=serde_json::from_str(include_str!("../../../../account/tests/fixtures/proxy-profile.json")).unwrap();
        profile["nodes"].as_array_mut().unwrap().truncate(1);
        profile["nodes"][0]["ingresses"].as_array_mut().unwrap().truncate(1);
        profile["routing"]=json!({"rule_sets":[],"rules":[],"final":{"type":"direct"}});
        for node in profile["nodes"].as_array_mut().unwrap() {for ingress in node["ingresses"].as_array_mut().unwrap() {ingress["endpoint"]=json!({"domain":"127.0.0.1","port":9});}}
        backend.call("/v1/apply-profile",json!({"profile":profile,"routing_mode":"rules"})).await.unwrap();
        backend.network(ppvpn_core::NetworkState::new("lo0", ppvpn_core::NetworkKind::Other)
            .with_addresses(vec!["127.0.0.1/8".into()])).unwrap();
        backend.call("/v1/start",json!({})).await.unwrap();
        let credential=backend.call("/v1/get-local-proxy-credential",json!({"kind":"routed"})).await.unwrap();
        let user=credential["username"].as_str().unwrap();let password=credential["password"].as_str().unwrap();let proxy=format!("127.0.0.1:{}",credential["port"].as_u64().unwrap());
        let identity=backend.identity();let gateway=Gateway::new(backend.clone(),501).unwrap();
        let mut denied=TcpStream::connect(&proxy).await.unwrap();denied.write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes()).await.unwrap();assert!(header(&mut denied).await.starts_with(b"HTTP/1.1 407"));drop(denied);
        let mut http=TcpStream::connect(&proxy).await.unwrap();let basic=base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"));
        http.write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: Basic {basic}\r\n\r\n").as_bytes()).await.unwrap();assert!(header(&mut http).await.starts_with(b"HTTP/1.1 200"));
        let mut socks=socks_auth(&proxy,user,password).await;let mut connect=vec![5,1,0,1,127,0,0,1];connect.extend_from_slice(&target.port().to_be_bytes());socks.write_all(&connect).await.unwrap();let _=socks_reply(&mut socks).await;
        let mut association=socks_auth(&proxy,user,password).await;association.write_all(&[5,3,0,1,0,0,0,0,0,0]).await.unwrap();let mut relay=socks_reply(&mut association).await;if relay.ip().is_unspecified() {relay.set_ip("127.0.0.1".parse().unwrap());}
        let udp_client=UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut epoch=0;
        for sequence in 0..10 {
            let attached=backend.call("/v1/attach-capture",json!({"protocol_version":1,"expected_epoch":epoch,"spec":{"resource":{"kind":"host_lease","lease_id":"isolated-packets"},"ipv6":true}})).await.unwrap();
            assert_eq!(attached["phase"],"attached");
            echo(&mut http,sequence).await;echo(&mut socks,sequence).await;
            let mut packet=vec![0,0,0,1,127,0,0,1];packet.extend_from_slice(&udp_target.port().to_be_bytes());packet.extend_from_slice(&sequence.to_be_bytes());udp_client.send_to(&packet,relay).await.unwrap();let mut reply=[0;2048];let(n,_)=udp_client.recv_from(&mut reply).await.unwrap();assert_eq!(&reply[n-4..n],&sequence.to_be_bytes());
            let detached=backend.call("/v1/detach-capture",json!({"protocol_version":1,"expected_epoch":attached["epoch"],"lease":attached["lease"]})).await.unwrap();
            assert_eq!(detached["phase"],"detached");epoch=detached["epoch"].as_u64().unwrap();
            echo(&mut http,sequence+100).await;echo(&mut socks,sequence+100).await;
            let after=backend.call("/v1/get-local-proxy-credential",json!({"kind":"routed"})).await.unwrap();assert!(credential==after);assert_eq!(identity,backend.identity());
        }
        drop(gateway);echo(&mut http,99).await;echo(&mut socks,99).await;
        drop((http,socks,association,udp_client));backend.shutdown().await.unwrap();tcp_task.abort();udp_task.abort();let _=std::fs::remove_dir_all(dir);
    }).await.expect("mixed traffic fixture must be bounded");
}
