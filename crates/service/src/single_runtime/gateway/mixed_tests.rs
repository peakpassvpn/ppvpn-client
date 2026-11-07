//! Real authenticated mixed traffic through one Standard engine. Capture calls
//! remain unsupported: this is NOT proof of native TUN attach/detach acceptance.
use super::*;
use base64::Engine as _;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
};
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
async fn authenticated_http_socks_tcp_udp_survive_refused_capture_and_ui_detach() {
    tokio::time::timeout(Duration::from_secs(30),async {
        let dir=std::env::temp_dir().join(format!("ppvpn-mixed-{}",uuid::Uuid::new_v4()));
        let tcp=TcpListener::bind("127.0.0.1:0").await.unwrap();let target=tcp.local_addr().unwrap();
        let tcp_task=tokio::spawn(async move {loop {let(mut socket,_)=tcp.accept().await.unwrap();tokio::spawn(async move {let(mut read,mut write)=socket.split();let _=tokio::io::copy(&mut read,&mut write).await;});}});
        let udp=UdpSocket::bind("127.0.0.1:0").await.unwrap();let udp_target=udp.local_addr().unwrap();
        let udp_task=tokio::spawn(async move {let mut packet=[0;2048];loop {let(n,peer)=udp.recv_from(&mut packet).await.unwrap();udp.send_to(&packet[..n],peer).await.unwrap();}});
        let backend=EngineForwarder::new(ppvpn_core::EngineConfig::new(ppvpn_core::Role::Standard,ppvpn_core::Platform::Macos,dir.clone()).with_local_proxy(ppvpn_core::LocalProxyConfig::new().with_preferred_port(0))).await.unwrap();
        let mut profile:Value=serde_json::from_str(include_str!("../../../../account/tests/fixtures/proxy-profile.json")).unwrap();
        profile["nodes"].as_array_mut().unwrap().truncate(1);
        profile["nodes"][0]["ingresses"].as_array_mut().unwrap().truncate(1);
        profile["routing"]=json!({"rule_sets":[],"rules":[],"final":{"type":"direct"}});
        for node in profile["nodes"].as_array_mut().unwrap() {for ingress in node["ingresses"].as_array_mut().unwrap() {ingress["endpoint"]=json!({"domain":"127.0.0.1","port":9});}}
        backend.call("/v1/apply-profile",json!({"profile":profile,"routing_mode":"rules"})).await.unwrap();
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
        for sequence in 0..10 {
            let result=backend.call("/v1/attach-capture",json!({"protocol_version":1,"expected_epoch":0,"spec":{"resource":{"kind":"host_lease","lease_id":"unaccepted"},"ipv6":true}})).await.unwrap_err();assert_eq!(result.code,"CAPTURE_UNSUPPORTED");
            echo(&mut http,sequence).await;echo(&mut socks,sequence).await;
            let mut packet=vec![0,0,0,1,127,0,0,1];packet.extend_from_slice(&udp_target.port().to_be_bytes());packet.extend_from_slice(&sequence.to_be_bytes());udp_client.send_to(&packet,relay).await.unwrap();let mut reply=[0;2048];let(n,_)=udp_client.recv_from(&mut reply).await.unwrap();assert_eq!(&reply[n-4..n],&sequence.to_be_bytes());
            let after=backend.call("/v1/get-local-proxy-credential",json!({"kind":"routed"})).await.unwrap();assert!(credential==after);assert_eq!(identity,backend.identity());
        }
        drop(gateway);echo(&mut http,99).await;echo(&mut socks,99).await;
        drop((http,socks,association,udp_client));backend.shutdown().await.unwrap();tcp_task.abort();udp_task.abort();let _=std::fs::remove_dir_all(dir);
    }).await.expect("mixed traffic fixture must be bounded");
}
