use crate::{Node, Peer};
use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

const PORT: u16 = 53318;
const GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 53, 18);

#[derive(Serialize, Deserialize)]
struct Beacon {
    magic: String,
    version: u8,
    id: String,
    name: String,
    port: u16,
    #[serde(default)]
    capabilities: Vec<String>,
}

pub fn addresses(port: u16) -> Vec<String> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|i| match i.addr {
            if_addrs::IfAddr::V4(a) if !a.ip.is_loopback() => {
                Some(SocketAddr::from((a.ip, port)).to_string())
            }
            _ => None,
        })
        .collect()
}

pub fn start(node: Arc<Node>) {
    tokio::spawn(async move {
        let result = run(node.clone()).await;
        if let Err(e) = result {
            node.warn(format!(
                "Automatic discovery unavailable: {e}. Connect by IP address."
            ));
        }
    });
}

async fn run(node: Arc<Node>) -> anyhow::Result<()> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    socket.set_broadcast(true)?;
    socket.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, PORT)).into())?;
    // Broadcast is primary; multicast is a supplementary path on Wi-Fi.
    let _ = socket.join_multicast_v4(&GROUP, &Ipv4Addr::UNSPECIFIED);
    socket.set_nonblocking(true)?;
    let socket = tokio::net::UdpSocket::from_std(socket.into())?;
    let mut tick = tokio::time::interval(Duration::from_secs(3));
    let mut buffer = [0u8; 2048];
    loop {
        if node.stopping.load(Ordering::SeqCst) {
            break;
        }
        tokio::select! {
            _ = node.shutdown.notified() => break,
            _ = tick.tick() => {
                let hello = node.hello();
                let bytes = serde_json::to_vec(&Beacon {
                    magic:"LOCAL_DISCOVERY".into(),
                    version:hello.version,
                    id:hello.id,
                    name:hello.name,
                    port:hello.port,
                    capabilities:hello.capabilities,
                })?;
                let mut targets = vec![Ipv4Addr::BROADCAST,GROUP];
                for iface in if_addrs::get_if_addrs().unwrap_or_default() {
                    if let if_addrs::IfAddr::V4(addr) = iface.addr {
                        if let Some(broadcast) = addr.broadcast {
                            if !targets.contains(&broadcast) { targets.push(broadcast); }
                        }
                    }
                }
                for target in targets { let _ = socket.send_to(&bytes,(target,PORT)).await; }
            },
            received = socket.recv_from(&mut buffer) => {
                let (len,source) = received?;
                if let Ok(beacon) = serde_json::from_slice::<Beacon>(&buffer[..len]) {
                    if beacon.magic!="LOCAL_DISCOVERY"
                        || beacon.version!=1
                        || beacon.id==node.id
                        || !crate::protocol::valid_hash(&beacon.id)
                        || beacon.port==0
                        || beacon.name.is_empty()
                        || beacon.name.len()>80
                        || beacon.name.chars().any(char::is_control)
                        || crate::protocol::validate_capabilities(&beacon.capabilities,None).is_err()
                    { continue; }
                    let capabilities = if beacon.capabilities.is_empty() {
                        crate::protocol::BASE_CAPABILITIES.iter().map(|value| (*value).to_owned()).collect()
                    } else {
                        beacon.capabilities
                    };
                    let mut peers = node.peers.lock().unwrap();
                    if peers.len() >= 256 && !peers.contains_key(&beacon.id) {
                        peers.retain(|_,p| crate::now()-p.last_seen<20_000);
                        if peers.len()>=256 { continue; }
                    }
                    peers.insert(beacon.id.clone(),Peer{
                        id:beacon.id,
                        name:beacon.name,
                        address:SocketAddr::new(source.ip(),beacon.port).to_string(),
                        last_seen:crate::now(),
                        connected:false,
                        trusted:false,
                        ready:false,
                        code:None,
                        local_confirmed:false,
                        capabilities,
                        capabilities_authenticated:false,
                        screen:None,
                    });
                }
            }
        }
    }
    Ok(())
}
