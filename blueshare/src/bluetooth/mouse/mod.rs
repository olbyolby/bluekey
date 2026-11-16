use std::sync::{Arc, Weak};

use bluer::{AdapterEvent, Address};
use bluer::gatt::local::{Characteristic, CharacteristicNotifier, CharacteristicNotifyFun};
use bluer::Adapter;
use futures::{FutureExt, StreamExt, pin_mut};
use log::{debug, info, warn};
use tokio::sync::{RwLock, broadcast, mpsc};

use crate::bluetooth::{DeviceMap, Register, ReturnEventListener};

use super::{Target, ServerError, Never};

use super::hid::{self, Notifiers, NotifyInbox, Protocol};
use super::hid::characteristics::callback;


pub(super) mod data;

#[derive(Clone, Copy, Debug)]
pub struct InvalidButton;
#[derive(Clone, Copy, Debug)]
pub struct Button {
    pub id: u16
}
impl Button {
    pub fn from_id(id: u16) -> Result<Self, InvalidButton> {
        match id {
            0 => Err(InvalidButton),
            _ => Ok(Button { id })
        }
    }
    pub fn into_u16(&self) -> u16 {
        self.id
    }
}
impl Into<u16> for Button {
    fn into(self) -> u16 {
        self.into_u16()
    }
}
impl TryFrom<u16> for Button {
    type Error = InvalidButton;
    fn try_from(value: u16) -> Result<Self, Self::Error> {
        Self::from_id(value)
    }
}


#[derive(Clone, Copy, Debug)]
pub struct MouseServerDied;

#[derive(Clone, Copy, Debug)]
pub enum TryMouseError {
    MouseServerDied,
    QueueFull
}

#[derive(Clone, Copy, Debug)]
enum MouseEvent {
    ButtonPress(Target, Button),
    ButtonRelease(Target, Button),
    Movement(Target, i8, i8, i8),
}
#[derive(Clone, Copy, Debug)]
pub enum MouseReturnEvent {
    Register(Address),
    Wake(Address),
    Suspend(Address)
}
impl From<Register> for MouseReturnEvent {
    fn from(value: Register) -> Self {
        MouseReturnEvent::Register(value.0)
    }
}

pub struct Mouse {
    channel: mpsc::Sender<MouseEvent>,
    _returns: broadcast::Receiver<MouseReturnEvent>,
    pub(super) returns_sender: broadcast::Sender<MouseReturnEvent>,
    handle: tokio::task::JoinHandle<Result<Never, ServerError>>,
    pub(super) state: Weak<RwLock<MouseServer>>,
    // Mouse specific GATT characteristics, taken once by bluetooth::service::serve
    characteristics: std::sync::Mutex<Option<Vec<Characteristic>>>
}

impl Mouse {
    pub fn new(adapter: Arc<Adapter>) -> Self {
        let (mouse_sender, mouse_receiver) = mpsc::channel(16);
        let (return_sender, return_receiver) = broadcast::channel(16);

        // Create the mouse server
        let state = Arc::new(RwLock::new(DeviceMap::new(return_sender.clone())));

        // The mouse's characteristics are served as part of the single, shared HID service(see bluetooth::service)
        let (boot_input_handle, boot_input_control) = hid::notify_channel();
        let (report_input_handle, report_input_control) = hid::notify_channel();
        let characteristics = mouse_characteristics(state.clone(), boot_input_handle, report_input_handle);

        let handle = tokio::spawn(mouse_server(mouse_receiver, adapter, state.clone(), boot_input_control, report_input_control).inspect(|error| {
            warn!("{}", error.as_ref().unwrap_err())
        }));

        Mouse {
            channel: mouse_sender,
            _returns: return_receiver,
            returns_sender: return_sender,
            state: Arc::downgrade(&state),
            handle,
            characteristics: std::sync::Mutex::new(Some(characteristics))
        }
    }

    // Hands the mouse's GATT characteristics to the shared HID service, can only be taken once
    pub(super) fn take_characteristics(&self) -> Vec<Characteristic> {
        self.characteristics.lock().unwrap().take().unwrap_or_default()
    }

    pub async fn press(&self, target: Target, button: Button) -> Result<(), MouseServerDied> {
        match self.channel.send(MouseEvent::ButtonPress(target, button)).await {
            Ok(_) => Ok(()),
            Err(mpsc::error::SendError(_)) => Err(MouseServerDied)
        }
    }

    pub fn try_press(&self, target: Target, button: Button) -> Result<(), TryMouseError> {
        match self.channel.try_send(MouseEvent::ButtonPress(target, button)) {
            Ok(_) => Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(TryMouseError::MouseServerDied),
            Err(mpsc::error::TrySendError::Full(_)) => Err(TryMouseError::QueueFull),
        }
    }
    pub async fn release(&self, target: Target, button: Button) -> Result<(), MouseServerDied> {
        match self.channel.send(MouseEvent::ButtonRelease(target, button)).await {
            Ok(_) => Ok(()),
            Err(mpsc::error::SendError(_)) => Err(MouseServerDied)
        }
    }

    pub fn try_release(&self, target: Target, button: Button) -> Result<(), TryMouseError> {
        match self.channel.try_send(MouseEvent::ButtonRelease(target, button)) {
            Ok(_) => Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(TryMouseError::MouseServerDied),
            Err(mpsc::error::TrySendError::Full(_)) => Err(TryMouseError::QueueFull),
        }
    }
    pub async fn moved(&self, target: Target, x: i8, y: i8, scroll: i8) -> Result<(), MouseServerDied> {
        match self.channel.send(MouseEvent::Movement(target, x, y, scroll)).await {
            Ok(_) => Ok(()),
            Err(mpsc::error::SendError(_)) => Err(MouseServerDied)
        }
    }

    pub fn try_moved(&self, target: Target, x: i8, y: i8, scroll: i8) -> Result<(), TryMouseError> {
        match self.channel.try_send(MouseEvent::Movement(target, x, y, scroll)) {
            Ok(_) => Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(TryMouseError::MouseServerDied),
            Err(mpsc::error::TrySendError::Full(_)) => Err(TryMouseError::QueueFull),
        }
    }

    pub fn listen(&self) -> ReturnEventListener<MouseReturnEvent> {
        ReturnEventListener { receiver: self.returns_sender.subscribe() }
    }

    pub fn cancel(&self) {
        self.handle.abort();
    }
    pub fn devices<'a>(&'a self,) -> Result<DevicesView, MouseServerDied> {
        Ok(DevicesView { state: self.state.upgrade().ok_or(MouseServerDied)? })

    }
}
impl Drop for Mouse {
    fn drop(&mut self) {
        self.handle.abort()
    }
}

pub struct DevicesView {
    state: Arc<RwLock<DeviceMap<IndividualMouse, MouseReturnEvent>>>
}
impl DevicesView {
    pub async fn collect<T: FromIterator<Address>>(&self) -> T {
        let lock = self.state.read().await;
        T::from_iter(lock.devices.keys().cloned())
    }
    pub async fn for_each<F: Fn(Address) -> ()>(&self, map: F) {
        let lock = self.state.read().await;
        for address in lock.devices.keys().cloned() {
            map(address);
        }
    }
}


pub(super) type MouseServer = DeviceMap<IndividualMouse, MouseReturnEvent>;
pub(super) struct IndividualMouse {
    pub(super) protocol: Protocol,
    buttons: u8
}
impl IndividualMouse {
    async fn send_report(&self, notifiers: &mut Notifiers, movement: Option<(i8, i8)>, wheel: Option<i8>) {
        let (mx, my) = movement.unwrap_or((0, 0));
        let wheel = wheel.unwrap_or(0);

        let report = match self.protocol {
            Protocol::Boot => &[
                self.buttons, // Current state of buttons
                mx as u8, // Preserve bit pattern
                my as u8,
            ] as &[u8],
            Protocol::Report => &[
                self.buttons, // Current state of buttons
                mx as u8, // Preserve bit pattern
                my as u8,
                wheel as u8
            ] as &[u8]
        };
        notifiers.send(self.protocol, report).await;
    }
}
impl Default for IndividualMouse {
    fn default() -> Self {
        IndividualMouse {
            protocol: Protocol::Report,
            buttons: 0
        }   
    }
}

const DEFAULT_STATE: IndividualMouse = IndividualMouse {
    protocol: Protocol::Report,
    buttons: 0
};

// Mouse specific characteristics of the HID service. The shared ones(protocol mode, HID information,
// control point and report map) are created by bluetooth::service, since there can only be one of each per service.
fn mouse_characteristics(state: Arc<RwLock<MouseServer>>, boot_input_handle: CharacteristicNotifyFun, report_input_handle: CharacteristicNotifyFun) -> Vec<Characteristic> {
    vec![
        hid::characteristics::boot_mouse_input(
            callback!(|request| state {
                let state = state.read().await;

                debug!("Boot report  read by {}", request.device_address);
                Ok(vec![state.get_device(request.device_address).unwrap_or(&DEFAULT_STATE).buttons, 0, 0])
            }),
            boot_input_handle
        ),
        hid::characteristics::input_report(
            callback!(|request| state {
                let state = state.read().await;

                debug!("Report read by {}", request.device_address);
                Ok(vec![state.get_device(request.device_address).unwrap_or(&DEFAULT_STATE).buttons, 0, 0, 0])
            }),
            report_input_handle,
            hid::MOUSE_REPORT_ID
        )
    ]
}

async fn mouse_server(mut receiever: mpsc::Receiver<MouseEvent>, adapter: Arc<Adapter>, state: Arc<RwLock<MouseServer>>, mut boot_input_control: NotifyInbox, mut report_input_control: NotifyInbox) -> Result<Never, ServerError> {
    let disconnect_events = adapter.events().await.expect("Couldn't monitor adapter events").filter_map(async |event|{
        match event {
            AdapterEvent::DeviceRemoved(address) => Some(address),
            _ => None
        }
    });
    drop(adapter);

    enum Event {
        BootReportNotify(CharacteristicNotifier),
        ReportNotify(CharacteristicNotifier),
        MouseEvent(MouseEvent),
        DeviceDisconnect(Address)
    }

    pin_mut!(disconnect_events);
    let mut events = async move || {
        tokio::select! {
            notifier = boot_input_control.next() => Ok::<Event, ServerError>(Event::BootReportNotify(notifier)),
            notifier = report_input_control.next() => Ok(Event::ReportNotify(notifier)),
            event = receiever.recv() => Ok(Event::MouseEvent(event.expect("??"))),
            event = disconnect_events.next() => Ok(Event::DeviceDisconnect(event.ok_or(ServerError::AdapterLost)?))
        }
    };

    let mut notifiers = Notifiers::new("mouse");

    info!("Successfully created mouse server");
    loop {
        let event = events().await?;
        let mut state = state.write().await;
        match event {
            Event::BootReportNotify(notifier) => notifiers.subscribed(Protocol::Boot, notifier),
            Event::ReportNotify(notifier) => notifiers.subscribed(Protocol::Report, notifier),
            Event::DeviceDisconnect(address) => {
                state.disconnect_device(address).await;
            },
            Event::MouseEvent(event) => match event {
                MouseEvent::ButtonPress(target, button) => {

                    for device in state.get_targets(target) {
                        // Subtract one from button ID to account for button 1 being at bit 0
                        if (1..=3).contains(&button.into_u16()) && device.buttons & (1<<(button.into_u16()-1)) == 0 {
                            device.buttons |= 1<<(button.into_u16()-1);
                            device.send_report(&mut notifiers, None, None).await;
                        }
                    }
                },
                MouseEvent::ButtonRelease(target, button) => {
                    for device in state.get_targets(target) {
                        // Subtract one from button ID to account for button 1 being at bit 0
                        if (1..=3).contains(&button.into_u16()) && device.buttons & (1<<(button.into_u16()-1)) != 0 {
                            device.buttons &= !(1<<(button.into_u16()-1));
                            device.send_report(&mut notifiers, None, None).await;
                        }
                    }

                },
                MouseEvent::Movement(target, x, y, scroll) => {
                    // Bridges report movement on every clock tick, even while the mouse sits still. Notifying the host
                    // of "no movement" 33 times a second uses airtime for nothing, and whenever the host's link slows
                    // down(busy radio, longer connection interval) those reports queue up in BlueZ and arrive late,
                    // making the pointer stall and then jump. Relative reports need no zero updates, so skip them.
                    if x == 0 && y == 0 && scroll == 0 {
                        continue;
                    }
                    for device in state.get_targets(target) {
                        device.send_report(&mut notifiers, Some((x, y)), Some(scroll)).await
                    }
                },
            }
        }
    }

}
