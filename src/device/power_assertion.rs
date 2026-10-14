use std::{net::IpAddr, path::Path, time::Duration};

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
    pub async fn new(
        addr: IpAddr,
        scope_id: Option<u32>,
        udid: &str,
        lockdown_path: &Path,
    ) -> Result<Self, RusbmuxError> {
        let assertion = Assertion::new(addr, scope_id, udid, lockdown_path).await?;

        info!(udid, "Holding a power assertion");

        let udid = udid.to_string();
        let lockdown_path = lockdown_path.to_path_buf();
        let renewal_handler = tokio::spawn(async move {
            let mut assertion = assertion;
            loop {
                tokio::time::sleep(RENEWAL_INTERVAL).await;

                let new_assertion =
                    match Assertion::new(addr, scope_id, &udid, &lockdown_path).await {
                        Ok(assertion) => assertion,
                        Err(err) => {
                            warn!(udid, %err, "Failed to renew the power assertion");
                            return;
                        }
                    };

                drop(assertion);
                assertion = new_assertion;
                debug!(udid, "Renewed the power assertion");
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
        udid: &str,
        lockdown_path: &Path,
    ) -> Result<Self, RusbmuxError> {
        let provider = TcpProvider {
            addr,
            scope_id,
            pairing_file: PairingFile::from_bytes(
                &tokio::fs::read(lockdown_path.join(format!("{udid}.plist"))).await?,
            )?,
            label: format!("rusbmux_{udid}_power_assertion"),
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
