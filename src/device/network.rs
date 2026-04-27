use std::net::{IpAddr, SocketAddr, SocketAddrV6};

use idevice::{
    IdeviceService, heartbeat::HeartbeatClient, pairing_file::PairingFile,
    provider::IdeviceProvider,
};
use tokio::{sync::watch, task::JoinHandle};
use tracing::{debug, warn};

use crate::{
    conn::NetworkDeviceConn, device::core::DeviceCore, error::RusbmuxError, handler::CONFIG_PATH,
    watcher::remove_device,
};

// TODO: remove once merged: https://github.com/jkcoxson/idevice/pull/85
#[derive(Debug)]
pub struct Tcpv6Provider {
    /// IP address of the device
    pub addr: std::net::Ipv6Addr,
    pub scope_id: u32,
    /// Pairing file for secure communication
    pub pairing_file: PairingFile,
    /// Label identifying this connection
    pub label: String,
}

impl idevice::provider::IdeviceProvider for Tcpv6Provider {
    /// Connects to the device over TCP
    ///
    /// # Arguments
    /// * `port` - The TCP port to connect to
    ///
    /// # Returns
    /// An `Idevice` wrapped in a future
    fn connect(
        &self,
        port: u16,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<idevice::Idevice, idevice::IdeviceError>> + Send>,
    > {
        let addr = self.addr;
        let label = self.label.clone();
        let scope_id = self.scope_id;
        Box::pin(async move {
            let socket_addr =
                std::net::SocketAddr::V6(std::net::SocketAddrV6::new(addr, port, 0, scope_id));
            let stream = tokio::net::TcpStream::connect(socket_addr).await?;
            Ok(idevice::Idevice::new(Box::new(stream), label))
        })
    }

    /// Returns the connection label
    fn label(&self) -> &str {
        &self.label
    }

    /// Returns the pairing file (cloned from the provider)
    fn get_pairing_file(
        &self,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<PairingFile, idevice::IdeviceError>> + Send>>
    {
        let pairing_file = self.pairing_file.clone();
        Box::pin(async move { Ok(pairing_file) })
    }
}

#[derive(Debug)]
pub struct NetworkDevice {
    pub core: DeviceCore,

    pub addr: IpAddr,

    /// In which interface this device got discovered (only useful for IPv6)
    pub scope_id: Option<u32>,

    pub mac_address: String,
    pub service_name: String,

    pub serial_number: String,

    pub hb_failed: watch::Receiver<()>,
    pub hb_handler: JoinHandle<()>,
}

impl Drop for NetworkDevice {
    fn drop(&mut self) {
        self.hb_handler.abort();

        self.shutdown();
    }
}

impl NetworkDevice {
    pub async fn new(
        id: u64,
        addr: IpAddr,
        scope_id: Option<u32>,
        mac_address: String,
        service_name: String,
        serial_number: String,
    ) -> Result<Self, RusbmuxError> {
        let mut heartbeat_client =
            Self::connect_heartbeat_client(addr, scope_id, serial_number.clone()).await?;

        let (tx, rx) = watch::channel(());

        let core = DeviceCore::new(id);

        let device_shutdown = core.canceler.clone();
        let hb_handler = tokio::spawn(async move {
            let mut interval = 15;

            let mut retries = 3;

            loop {
                interval = match heartbeat_client.get_marco(interval).await {
                    Ok(i) => i + 5,
                    Err(e) => {
                        if retries == 0 {
                            warn!(id, "Heartbeat failed, error: {e}, closing device");
                            let _ = tx.send(());
                            device_shutdown.cancel();
                            let _ = remove_device(id).await;
                            return;
                        }

                        warn!(id, "Heartbeat failed, error: {e}, retrying");
                        retries -= 1;
                        interval
                    }
                };
                if let Err(e) = heartbeat_client.send_polo().await {
                    if retries == 0 {
                        warn!(id, "Heartbeat failed, error: {e}, closing device");
                        let _ = tx.send(());
                        device_shutdown.cancel();
                        let _ = remove_device(id).await;
                        return;
                    }

                    warn!(id, "Heartbeat failed, error: {e}, retrying");
                    retries -= 1;
                }
            }
        });

        Ok(Self {
            core,
            addr,
            scope_id,
            service_name,
            mac_address,
            serial_number,
            hb_failed: rx,
            hb_handler,
        })
    }

    async fn connect_heartbeat_client(
        addr: IpAddr,
        scope_id: Option<u32>,
        serial_number: String,
    ) -> Result<HeartbeatClient, RusbmuxError> {
        let pairing_file =
            PairingFile::read_from_file(format!("{CONFIG_PATH}/lockdown/{serial_number}.plist"))?;

        let label = format!("rusbmux_{serial_number}_heartbeat_client");

        let tcp: Box<dyn IdeviceProvider> = match addr {
            IpAddr::V4(_) => Box::new(idevice::provider::TcpProvider {
                addr,
                pairing_file,
                label,
            }),
            IpAddr::V6(ipv6) => Box::new(Tcpv6Provider {
                addr: ipv6,
                scope_id: scope_id.unwrap_or(0),
                pairing_file,
                label,
            }),
        };

        Ok(HeartbeatClient::connect(tcp.as_ref()).await?)
    }

    pub async fn connect(&self, port: u16) -> Result<NetworkDeviceConn, RusbmuxError> {
        debug!(
            device_id = self.core.id,
            dst_port = port,
            "Creating new connection"
        );

        let socket = match self.addr {
            IpAddr::V4(_) => SocketAddr::new(self.addr, port),
            IpAddr::V6(ipv6) => {
                SocketAddr::V6(SocketAddrV6::new(ipv6, port, 0, self.scope_id.unwrap_or(0)))
            }
        };
        NetworkDeviceConn::new(socket, self.core.id, self.core.canceler.clone()).await
    }

    #[inline]
    pub fn shutdown(&self) {
        self.core.canceler.cancel();
    }
}

impl NetworkDevice {
    pub fn create_device_attached(&self) -> Result<plist::Value, RusbmuxError> {
        let mut network_address = [0u8; 128];
        match self.addr {
            IpAddr::V4(ipv4) => {
                let family = libc::AF_INET as u16;

                network_address[..2].copy_from_slice(&family.to_ne_bytes());
                network_address[2..4].copy_from_slice(&0u16.to_ne_bytes());
                network_address[4..8].copy_from_slice(&ipv4.octets());
            }
            IpAddr::V6(ipv6) => {
                let family = libc::AF_INET6 as u16;

                network_address[..2].copy_from_slice(&family.to_ne_bytes());
                network_address[2..4].copy_from_slice(&0u16.to_ne_bytes());
                network_address[2..4].copy_from_slice(&0u16.to_ne_bytes());
                network_address[8..24].copy_from_slice(&ipv6.octets());
                network_address[24..28].copy_from_slice(&self.scope_id.unwrap_or(0).to_ne_bytes());
            }
        }

        Ok(plist_macro::plist!({
            "MessageType": "Attached",
            "DeviceID": self.core.id,
            "Properties": {
                "DeviceID": self.core.id,
                "ConnectionType": "Network",

                "EscapedFullServiceName": &self.service_name,
                "InterfaceIndex": self.scope_id.unwrap_or(0),
                "NetworkAddress": network_address.to_vec(),
                "SerialNumber": &self.serial_number
            }
        }))
    }
}
