use std::{net::IpAddr, time::Duration};

use idevice::{
    Idevice, IdeviceError, IdeviceService, pairing_file::PairingFile, provider::TcpProvider,
};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::error::RusbmuxError;

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
    pub fn new(
        addr: IpAddr,
        scope_id: Option<u32>,
        pairing_file: PairingFile,
    ) -> Result<Self, RusbmuxError> {
        // TODO: move it inside of the heartbeat; so it's one spawn
        let renewal_handler = tokio::spawn(async move {
            let provider = TcpProvider {
                addr,
                scope_id,
                label: "rusbmux_power_assertion".to_string(),
                pairing_file,
            };

            let udid = provider.pairing_file.udid.as_deref();

            info!(udid, "Holding a power assertion");
            loop {
                let mut assertion = match Assertion::connect(&provider).await {
                    Ok(a) => a,
                    Err(err) => {
                        warn!(udid, %err, "Failed to renew the power assertion");
                        return;
                    }
                };

                if let Err(err) = assertion.assert().await {
                    warn!(udid, %err, "Failed to renew the power assertion");
                    return;
                }

                debug!(udid, "Renewed the power assertion");

                tokio::time::sleep(RENEWAL_INTERVAL).await;
            }
        });

        Ok(Self { renewal_handler })
    }
}

#[derive(Debug)]
struct Assertion(Idevice);

impl Assertion {
    async fn assert(&mut self) -> Result<(), RusbmuxError> {
        let reply = self
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

        Ok(())
    }

    async fn send_recv(
        &mut self,
        message: plist::Value,
    ) -> Result<plist::Dictionary, RusbmuxError> {
        let mut body = Vec::new();
        message.to_writer_xml(&mut body)?;

        let mut frame = Vec::with_capacity(body.len() + 4);

        frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
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
