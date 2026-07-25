use std::{net::IpAddr, time::Duration};

use idevice::{
    Idevice, IdeviceError, IdeviceService, pairing_file::PairingFile, provider::TcpProvider,
};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::{error::RusbmuxError, handler::LOCKDOWN_PATH};

const ASSERTION_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const RENEWAL_INTERVAL: Duration = Duration::from_secs(10 * 60);

#[derive(Debug)]
pub struct PowerAssertion {
    renewal_handler: JoinHandle<()>,
}

impl Drop for PowerAssertion {
    fn drop(&mut self) {
        self.renewal_handler.abort();
    }
}

impl PowerAssertion {
    pub async fn new(
        addr: IpAddr,
        scope_id: Option<u32>,
        serial_number: &str,
    ) -> Result<Self, RusbmuxError> {
        let assertion = Assertion::new(addr, scope_id, serial_number).await?;

        info!(serial_number, "Holding a power assertion");

        let serial_number = serial_number.to_string();
        let renewal_handler = tokio::spawn(async move {
            let mut assertion = assertion;
            loop {
                tokio::time::sleep(RENEWAL_INTERVAL).await;

                let new_assertion = match Assertion::new(addr, scope_id, &serial_number).await {
                    Ok(assertion) => assertion,
                    Err(error) => {
                        warn!(serial_number, %error, "Failed to renew the power assertion");
                        return;
                    }
                };

                drop(assertion);
                assertion = new_assertion;
                debug!(serial_number, "Renewed the power assertion");
            }
        });

        Ok(Self { renewal_handler })
    }
}

#[derive(Debug)]
struct Assertion(Idevice);

impl Assertion {
    async fn new(
        addr: IpAddr,
        scope_id: Option<u32>,
        serial_number: &str,
    ) -> Result<Self, RusbmuxError> {
        let provider = TcpProvider {
            addr,
            scope_id,
            pairing_file: PairingFile::read_from_file(format!(
                "{LOCKDOWN_PATH}/{serial_number}.plist"
            ))?,
            label: format!("rusbmux_{serial_number}_power_assertion"),
        };
        let mut assertion = Self::connect(&provider).await?;

        let reply = assertion
            .send_recv(plist_macro::plist!({
                "CommandKey": "CommandCreateAssertion",
                "AssertionTypeKey": "AMDPowerAssertionTypeWirelessSync",
                "AssertionNameKey": "rusbmux",
                "AssertionTimeoutKey": ASSERTION_TIMEOUT.as_secs_f64(),
                "AssertionDetailKey": "rusbmux",
            }))
            .await?;

        if let Some(error) = reply.get("ErrorKey") {
            return Err(RusbmuxError::PowerAssertion(format!("{error:?}")));
        }

        Ok(assertion)
    }

    async fn send_recv(
        &mut self,
        message: plist::Value,
    ) -> Result<plist::Dictionary, RusbmuxError> {
        let mut body = Vec::new();
        message.to_writer_xml(&mut body)?;

        let mut frame = (body.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(&body);
        self.0.send_raw(&frame).await?;

        let len = u32::from_be_bytes(self.0.read_raw(4).await?.try_into().unwrap());
        let body = self.0.read_raw(len as usize).await?;
        Ok(plist::from_bytes(&body)?)
    }
}

impl IdeviceService for Assertion {
    fn service_name() -> std::borrow::Cow<'static, str> {
        "com.apple.mobile.assertion_agent".into()
    }

    async fn from_stream(idevice: Idevice) -> Result<Self, IdeviceError> {
        Ok(Self(idevice))
    }
}
