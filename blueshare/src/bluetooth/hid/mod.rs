pub(super) mod characteristics;
pub(super) mod definitions;

use bluer::gatt::local::{CharacteristicNotifier, CharacteristicNotifyFun};
use log::{info, warn};
use tokio::sync::mpsc;

// Keyboard and mouse share a single HID service (Android only binds the first HID
// service it finds on a device), so each report needs its own Report ID.
pub(super) const KEYBOARD_REPORT_ID: u8 = 1;
pub(super) const MOUSE_REPORT_ID: u8 = 2;

pub(super) const HID_INFORMATION: &'static [u8] = &[
    0x01, 0x11, // HID spec
    0x00, // Country code
    0b00000010 //flags
];

#[derive(Clone, Copy, Debug)]
pub(super) enum Protocol {
    Boot,
    Report
}
impl Into<u8> for Protocol {
    fn into(self) -> u8 {
        match self {
            Self::Boot => 0,
            Self::Report => 1
        }
    }
}


// Notification sessions for one input(keyboard or mouse), one per protocol.
//
// These use BlueZ's StartNotify method rather than AcquireNotify. AcquireNotify hands out a socket per
// connected host and shuts it down whenever that host disconnects, and a bonded host(like Android) never
// subscribes again on reconnection, so input silently stopped after any reconnect. With StartNotify, BlueZ
// keeps a bonded host's subscription across disconnections and delivers every value update to it whenever
// it's connected, so a reconnecting phone just keeps working.
pub(super) struct Notifiers {
    name: &'static str,
    boot: Option<CharacteristicNotifier>,
    report: Option<CharacteristicNotifier>,
    reported_missing: bool
}
impl Notifiers {
    pub(super) fn new(name: &'static str) -> Self {
        Notifiers { name, boot: None, report: None, reported_missing: false }
    }

    pub(super) fn subscribed(&mut self, protocol: Protocol, notifier: CharacteristicNotifier) {
        info!("Host subscribed to {} {:?} notifications", self.name, protocol);
        self.reported_missing = false;
        match protocol {
            Protocol::Boot => self.boot = Some(notifier),
            Protocol::Report => self.report = Some(notifier)
        }
    }

    pub(super) async fn send(&mut self, protocol: Protocol, data: &[u8]) {
        let slot = match protocol {
            Protocol::Boot => &mut self.boot,
            Protocol::Report => &mut self.report
        };
        let error = match slot.as_mut() {
            Some(notifier) => match notifier.notify(data.to_vec()).await {
                Ok(()) => return,
                Err(error) => error
            },
            None => {
                if !self.reported_missing {
                    warn!("No host is subscribed to {} {:?} notifications, input is dropped until one subscribes(re-pair the device if this persists)", self.name, protocol);
                    self.reported_missing = true;
                }
                return
            }
        };
        // Every host unsubscribed(BlueZ called StopNotify). Never crash over it, a new session starts when one subscribes.
        warn!("Sending {} {:?} notification failed({}), waiting for a host to subscribe again", self.name, protocol, error);
        *slot = None;
    }
}

// Receives the notification sessions BlueZ starts(StartNotify) for one characteristic
pub(super) struct NotifyInbox {
    rx: mpsc::UnboundedReceiver<CharacteristicNotifier>,
    // Keeps the channel open even once the characteristic is gone, so next() never spins on a closed channel
    _keepalive: mpsc::UnboundedSender<CharacteristicNotifier>
}
impl NotifyInbox {
    pub(super) async fn next(&mut self) -> CharacteristicNotifier {
        self.rx.recv().await.expect("NotifyInbox holds a sender, so its channel never closes")
    }
}

// The notify function for a characteristic, and the inbox its sessions arrive in. The function returns
// immediately, since BlueZ waits for it before answering the host's subscription.
pub(super) fn notify_channel() -> (CharacteristicNotifyFun, NotifyInbox) {
    let (tx, rx) = mpsc::unbounded_channel();
    let forward = tx.clone();
    let fun: CharacteristicNotifyFun = Box::new(move |notifier| {
        let _ = forward.send(notifier);
        Box::pin(async {})
    });
    (fun, NotifyInbox { rx, _keepalive: tx })
}
