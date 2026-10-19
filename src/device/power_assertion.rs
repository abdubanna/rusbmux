use std::{net::IpAddr, time::Duration};

use idevice::{
    Idevice, IdeviceError, IdeviceService, pairing_file::PairingFile, provider::TcpProvider,
};
use tracing::{debug, warn};

use crate::error::RusbmuxError;

const ASSERTION_TIMEOUT: Duration = Duration::from_secs(20 * 60);
pub const RENEWAL_INTERVAL: Duration = Duration::from_secs(10 * 60);

#[derive(Debug)]
pub struct PowerAssertion {
    provider: TcpProvider,
    assertion: Option<Assertion>,
}

impl PowerAssertion {
    pub fn new(addr: IpAddr, scope_id: Option<u32>, pairing_file: PairingFile) -> Self {
        Self {
            provider: TcpProvider {
                addr,
                scope_id,
                label: "rusbmux_power_assertion".to_string(),
                pairing_file,
            },
            assertion: None,
        }
    }

    /// (re)connects the assertion channel; returns false if it had to give up, in which case
    /// the caller should stop asking (a device that refuses assertions is unreachable anyway)
    pub async fn renew(&mut self) -> bool {
        let udid = self.provider.pairing_file.udid.as_deref();

        let mut assertion = match Assertion::connect(&self.provider).await {
            Ok(assertion) => assertion,
            Err(err) => {
                warn!(udid, %err, "Failed to connect to the power assertion service");
                self.assertion = None;
                return false;
            }
        };

        match assertion.assert().await {
            Ok(()) => {
                debug!(udid, "Renewed the power assertion");
                // so it won't drop the connection
                self.assertion = Some(assertion);
                true
            }
            Err(err) => {
                warn!(udid, %err, "Failed to renew the power assertion");
                self.assertion = None;
                true
            }
        }
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
