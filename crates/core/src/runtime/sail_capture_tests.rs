//! Native dynamic capture through the production Runtime adapter. Loopback
//! sockets and in-memory IP packets only: no OS TUN/routes/DNS changes.
use super::*;
use base64::Engine as _;
use tokio::net::UdpSocket;

const TUN: &str = r#"{"type":"tun","tag":"capture","address":["10.233.0.1/24"],"mtu":1500}"#;

async fn associate(port: u16, password: &str) -> (TcpStream, SocketAddr) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(&[5, 1, 2]).await.unwrap();
    let mut answer = [0; 2];
    s.read_exact(&mut answer).await.unwrap();
    assert_eq!(answer, [5, 2]);
    let mut auth = vec![1, 1, b'u', password.len() as u8];
    auth.extend(password.as_bytes());
    s.write_all(&auth).await.unwrap();
    s.read_exact(&mut answer).await.unwrap();
    assert_eq!(answer, [1, 0]);
    s.write_all(&[5, 3, 0, 1, 0, 0, 0, 0, 0, 0]).await.unwrap();
    let mut reply = [0; 10];
    s.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply[1], 0);
    (
        s,
        SocketAddr::from((
            [reply[4], reply[5], reply[6], reply[7]],
            u16::from_be_bytes([reply[8], reply[9]]),
        )),
    )
}

async fn udp_round_trip(client: &UdpSocket, relay: SocketAddr, target: SocketAddr) {
    let SocketAddr::V4(target) = target else {
        unreachable!()
    };
    let mut packet = vec![0, 0, 0, 1];
    packet.extend(target.ip().octets());
    packet.extend(target.port().to_be_bytes());
    packet.extend(b"same-mixed-association");
    client.send_to(&packet, relay).await.unwrap();
    let mut answer = [0; 256];
    let (n, _) = tokio::time::timeout(WAIT, client.recv_from(&mut answer))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&answer[10..n], b"same-mixed-association");
}

/// Minimal IPv4/UDP packet, checksum zero permitted for IPv4 UDP. Packets
/// stay in the public bridge; no OS device or external network is involved.
fn captured_udp(target: SocketAddr) -> Vec<u8> {
    let SocketAddr::V4(target) = target else {
        unreachable!()
    };
    let payload = b"captured-via-runtime";
    let mut packet = vec![0_u8; 28 + payload.len()];
    packet[0] = 0x45;
    let length = packet.len() as u16;
    packet[2..4].copy_from_slice(&length.to_be_bytes());
    packet[8] = 64;
    packet[9] = 17;
    packet[12..16].copy_from_slice(&[10, 233, 0, 2]);
    packet[16..20].copy_from_slice(&target.ip().octets());
    packet[20..22].copy_from_slice(&42000_u16.to_be_bytes());
    packet[22..24].copy_from_slice(&target.port().to_be_bytes());
    packet[24..26].copy_from_slice(&((payload.len() + 8) as u16).to_be_bytes());
    packet[28..].copy_from_slice(payload);
    let mut sum: u32 = packet[..20]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u32::from(u16::from_be_bytes([p[0], p[1]])))
        .sum();
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    packet[10..12].copy_from_slice(&(!(sum as u16)).to_be_bytes());
    packet
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_capture_preserves_authenticated_tcp_http_udp_and_owner() {
    let runtime = SailRuntime::with_dynamic_capture(options("native-capture-continuity")).unwrap();
    assert!(runtime.has_dynamic_capture());
    let port = free_port();
    let secret = password();
    runtime
        .start(&config(port, &[("u", &secret)], false))
        .await
        .unwrap();
    runtime.select("pick", "direct-b").await.unwrap();
    assert_eq!(
        runtime
            .groups()
            .await
            .unwrap()
            .into_iter()
            .find(|g| g.tag == "pick")
            .unwrap()
            .now,
        "direct-b"
    );
    let tcp_target = echo().await;
    let udp_server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let udp_target = udp_server.local_addr().unwrap();
    let udp_task = tokio::spawn(async move {
        let mut bytes = [0; 2048];
        while let Ok((n, peer)) = udp_server.recv_from(&mut bytes).await {
            let _ = udp_server.send_to(&bytes[..n], peer).await;
        }
    });
    let mut socks_tcp = socks(port, "u", &secret, tcp_target).await.unwrap();
    let mut http = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("u:{secret}"));
    http.write_all(format!("CONNECT {tcp_target} HTTP/1.1\r\nHost: {tcp_target}\r\nProxy-Authorization: Basic {auth}\r\n\r\n").as_bytes()).await.unwrap();
    let mut head = Vec::new();
    loop {
        head.push(
            tokio::time::timeout(WAIT, http.read_u8())
                .await
                .unwrap()
                .unwrap(),
        );
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
        assert!(head.len() < 4096);
    }
    assert!(head.starts_with(b"HTTP/1.1 200"));
    let (_association, relay) = associate(port, &secret).await;
    let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    round_trip(&mut socks_tcp, b"before-capture").await;
    round_trip(&mut http, b"before-http-capture").await;
    udp_round_trip(&udp, relay, udp_target).await;
    let owner = runtime.observation();
    let mixed_ids: Vec<_> = runtime
        .connections()
        .await
        .unwrap()
        .iter()
        .map(|c| c.id)
        .collect();
    assert!(mixed_ids.len() >= 3);
    let mut traffic = runtime.traffic().await.unwrap();
    for _ in 0..3 {
        let (prepared, mut flow) = runtime.prepare_capture_packets(TUN, 2).await.unwrap();
        assert!(matches!(
            runtime.native_capture_state().unwrap(),
            embed::TunState::Prepared { .. }
        ));
        prepared.commit().await.unwrap();
        assert!(matches!(
            runtime.native_capture_state().unwrap(),
            embed::TunState::Attached { .. }
        ));
        flow.uplink.send(captured_udp(udp_target)).await.unwrap();
        let wire = tokio::time::timeout(WAIT, flow.downlink.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&wire[16..20], &[10, 233, 0, 2]);
        let header = usize::from(wire[0] & 0x0f) * 4;
        assert_eq!(&wire[header + 2..header + 4], &42000_u16.to_be_bytes());
        assert_eq!(&wire[header + 8..], b"captured-via-runtime");
        round_trip(&mut socks_tcp, b"attached").await;
        round_trip(&mut http, b"attached-http").await;
        udp_round_trip(&udp, relay, udp_target).await;
        let before_detach = runtime.traffic().await.unwrap();
        assert!(runtime.detach_native_capture().await.unwrap().clean());
        let after_detach = runtime.traffic().await.unwrap();
        assert!(
            after_detach.upload_bytes >= before_detach.upload_bytes
                && after_detach.download_bytes >= before_detach.download_bytes,
            "detach cannot reset totals before any new mixed traffic"
        );
        assert_eq!(
            runtime
                .groups()
                .await
                .unwrap()
                .into_iter()
                .find(|g| g.tag == "pick")
                .unwrap()
                .now,
            "direct-b"
        );
        assert!(matches!(
            runtime.native_capture_state().unwrap(),
            embed::TunState::Detached
        ));
        assert!(flow.uplink.send(vec![0x45]).await.is_err());
        assert!(flow.downlink.recv().await.is_none());
        round_trip(&mut socks_tcp, b"detached").await;
        round_trip(&mut http, b"detached-http").await;
        udp_round_trip(&udp, relay, udp_target).await;
        let current = runtime.observation();
        assert_eq!(current.owner, owner.owner);
        assert_eq!(current.native_runtime_id, owner.native_runtime_id);
        assert!(current.running);
        let ids: Vec<_> = runtime
            .connections()
            .await
            .unwrap()
            .iter()
            .map(|c| c.id)
            .collect();
        assert!(mixed_ids.iter().all(|id| ids.contains(id)));
        let now = runtime.traffic().await.unwrap();
        assert!(
            now.upload_bytes >= traffic.upload_bytes
                && now.download_bytes >= traffic.download_bytes
        );
        traffic = now;
        let verified = socks(port, "u", &secret, tcp_target).await;
        assert!(
            verified.is_some(),
            "original credentials still authenticate"
        );
        drop(verified);
        let mut denied = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        denied.write_all(&[5, 1, 0]).await.unwrap();
        let mut refusal = [0; 2];
        tokio::time::timeout(WAIT, denied.read_exact(&mut refusal))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(refusal, [5, 255], "capture never disables authentication");
    }
    assert!(runtime.detach_native_capture().await.unwrap().clean());
    runtime.stop().await.unwrap();
    udp_task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_capture_prepare_rollback_and_stale_commit_do_not_restart() {
    let runtime = SailRuntime::with_dynamic_capture(options("native-capture-rollback")).unwrap();
    let port = free_port();
    let secret = password();
    let cfg = config(port, &[("u", &secret)], false);
    runtime.start(&cfg).await.unwrap();
    let before = runtime.observation();
    let (prepared, mut flow) = runtime.prepare_capture_packets(TUN, 1).await.unwrap();
    drop(prepared);
    assert!(flow.downlink.recv().await.is_none());
    assert!(matches!(
        runtime.native_capture_state().unwrap(),
        embed::TunState::Detached
    ));
    assert_eq!(runtime.observation().owner, before.owner);
    assert!(runtime.prepare_capture_packets(TUN, 0).await.is_err());
    let (stale, _) = runtime.prepare_capture_packets(TUN, 1).await.unwrap();
    runtime.stop().await.unwrap();
    runtime.start(&cfg).await.unwrap();
    assert!(stale.commit().await.is_err());
    assert!(matches!(
        runtime.native_capture_state().unwrap(),
        embed::TunState::Detached
    ));
    assert_eq!(
        runtime.observation().owner.generation,
        before.owner.generation + 1
    );
    runtime.stop().await.unwrap();
}
