use std::{
    collections::{HashMap, VecDeque},
    pin::Pin,
    task::{Context, Poll},
};

use futures::{Stream, StreamExt};

use crate::usb_backend::APPLE_VID;

pub(super) enum UsbEvent {
    Arrived(::rusb::Device<::rusb::GlobalContext>),
    Left(::rusb::Device<::rusb::GlobalContext>),
}

pub(super) struct PollingStream {
    rx: tokio::sync::mpsc::UnboundedReceiver<UsbEvent>,
}

impl PollingStream {
    fn new() -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        tokio::spawn(async move {
            let mut known: HashMap<u64, rusb::Device<rusb::GlobalContext>> = HashMap::new();
            let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(300));
            let mut pending = VecDeque::new();

            loop {
                interval.tick().await;

                if let Some(event) = pending.pop_front() {
                    if tx.send(event).is_err() {
                        break;
                    }
                    continue;
                }

                let Ok(devices) = rusb::devices() else {
                    continue;
                };

                let mut current = HashMap::new();

                for dev in devices.iter() {
                    let Ok(desc) = dev.device_descriptor() else {
                        continue;
                    };

                    if desc.vendor_id() != APPLE_VID {
                        continue;
                    }

                    current.insert(super::opaque_id(&dev), dev);
                }

                for (&id, dev) in &current {
                    if known.contains_key(&id) {
                        continue;
                    }

                    pending.push_back(UsbEvent::Arrived(dev.clone()));
                }

                for (id, dev) in &known {
                    if current.contains_key(id) {
                        continue;
                    }

                    pending.push_back(UsbEvent::Left(dev.clone()));
                }

                known = current;
            }
        });

        Self { rx }
    }
}

impl Stream for PollingStream {
    type Item = UsbEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

pub(super) struct HotplugStream {
    _registration: ::rusb::Registration<::rusb::GlobalContext>,
    rx: tokio::sync::mpsc::UnboundedReceiver<UsbEvent>,
}

impl HotplugStream {
    fn new() -> rusb::Result<Self> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        let registration = ::rusb::HotplugBuilder::new()
            .enumerate(true)
            .vendor_id(APPLE_VID)
            .register(::rusb::GlobalContext::default(), Box::new(Callback { tx }))?;

        Ok(Self {
            _registration: registration,
            rx,
        })
    }
}

struct Callback {
    tx: tokio::sync::mpsc::UnboundedSender<UsbEvent>,
}

impl ::rusb::Hotplug<::rusb::GlobalContext> for Callback {
    fn device_arrived(&mut self, device: ::rusb::Device<::rusb::GlobalContext>) {
        let _ = self.tx.send(UsbEvent::Arrived(device));
    }

    fn device_left(&mut self, device: ::rusb::Device<::rusb::GlobalContext>) {
        let _ = self.tx.send(UsbEvent::Left(device));
    }
}

impl Stream for HotplugStream {
    type Item = UsbEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

pub(super) enum DeviceHotplugStream {
    Hotplug(HotplugStream),
    Polling(PollingStream),
}

impl DeviceHotplugStream {
    pub fn new() -> rusb::Result<Self> {
        if rusb::has_hotplug() {
            Ok(Self::Hotplug(HotplugStream::new()?))
        } else {
            Ok(Self::Polling(PollingStream::new()))
        }
    }
}

impl Stream for DeviceHotplugStream {
    type Item = UsbEvent;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.get_mut() {
            Self::Hotplug(hotplug) => hotplug.poll_next_unpin(cx),
            Self::Polling(polling) => polling.poll_next_unpin(cx),
        }
    }
}
