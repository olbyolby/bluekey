use std::sync::Arc;

use bluer::gatt::CharacteristicWriter;
use bluer::gatt::local::{Application, CharacteristicControlEvent, ReqError, Service};
use bluer::{Adapter, adv::Advertisement, gatt::local::characteristic_control};
use futures::StreamExt;
use log::debug;
use tokio::sync::{RwLock, mpsc};

use super::hid::{self, Protocol};
use super::hid::characteristics::callback;

mod data;

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
    ButtonPress(Button),
    ButtonRelease(Button),
    Movement(i8, i8)
}
#[derive(Clone, Copy, Debug)]
pub enum MouseReturnEvent {
    Wake,
    Suspend
}

pub struct Mouse {
    channel: mpsc::Sender<MouseEvent>,
    returns: mpsc::Receiver<MouseReturnEvent>,
    handle: tokio::task::JoinHandle<()>
}
#[allow(dead_code)]
impl Mouse {
    pub fn new(adapter: Arc<Adapter>) -> Self {
        let (mouse_sender, mouse_receiver) = mpsc::channel(16);
        let (return_sender, return_receiver) = mpsc::channel(16);

        // Create the mouse server
        let handle = tokio::spawn(mouse_server(mouse_receiver, return_sender, adapter));

        Mouse {
            channel: mouse_sender,
            returns: return_receiver,
            handle
        }
    }

    pub async fn press(&self, button: Button) -> Result<(), MouseServerDied> {
        match self.channel.send(MouseEvent::ButtonPress(button)).await {
            Ok(_) => Ok(()),
            Err(mpsc::error::SendError(_)) => Err(MouseServerDied)
        }
    }

    pub fn try_press(&self, button: Button) -> Result<(), TryMouseError> {
        match self.channel.try_send(MouseEvent::ButtonPress(button)) {
            Ok(_) => Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(TryMouseError::MouseServerDied),
            Err(mpsc::error::TrySendError::Full(_)) => Err(TryMouseError::QueueFull),
        }
    }
    pub async fn release(&self, button: Button) -> Result<(), MouseServerDied> {
        match self.channel.send(MouseEvent::ButtonRelease(button)).await {
            Ok(_) => Ok(()),
            Err(mpsc::error::SendError(_)) => Err(MouseServerDied)
        }
    }

    pub fn try_release(&self, button: Button) -> Result<(), TryMouseError> {
        match self.channel.try_send(MouseEvent::ButtonRelease(button)) {
            Ok(_) => Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(TryMouseError::MouseServerDied),
            Err(mpsc::error::TrySendError::Full(_)) => Err(TryMouseError::QueueFull),
        }
    }
    pub async fn moved(&self, x: i8, y: i8) -> Result<(), MouseServerDied> {
        match self.channel.send(MouseEvent::Movement(x, y)).await {
            Ok(_) => Ok(()),
            Err(mpsc::error::SendError(_)) => Err(MouseServerDied)
        }
    }
    
    pub fn try_moved(&self, x: i8, y: i8) -> Result<(), TryMouseError> {
        match self.channel.try_send(MouseEvent::Movement(x, y)) {
            Ok(_) => Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(TryMouseError::MouseServerDied),
            Err(mpsc::error::TrySendError::Full(_)) => Err(TryMouseError::QueueFull),
        }
    }

    pub async fn next_event(&mut self) -> Result<MouseReturnEvent, MouseServerDied> {
        match self.returns.recv().await {
            Some(event) => Ok(event),
            None => Err(MouseServerDied)
        }
    }

    pub fn cancel(&self) {
        self.handle.abort();
    }
}


struct MouseState {
    protocol: Protocol,
    buttons: u8,

    report: Vec<CharacteristicWriter>,
    boot: Vec<CharacteristicWriter>
}
impl Default for MouseState {
    fn default() -> Self {
        MouseState { 
            protocol: Protocol::Boot,
            buttons: 0,

            report: Vec::new(),
            boot: Vec::new()
        }
    }
}
impl MouseState {
    async fn send_report(&mut self, movement: Option<(i8, i8)>) {
        let (mx, my) = movement.unwrap_or((0, 0));

        let report = [
            self.buttons, // Current state of buttons
            mx as u8, // Preserve bit pattern
            my as u8,
        ];

        //println!("Sending report {:?} on protocol {:?}", report, self.protocol);
        let notifiers = match self.protocol {
            Protocol::Boot => &mut self.boot,
            Protocol::Report => &mut self.report
        };

        // Remove dead nodifiers
        notifiers.retain(|l| !l.is_closed().unwrap_or(true)); // I fear is checking if the stream is closed errors, it's probably closed
        for notifier in notifiers {
            if let Err(err) = notifier.send(&report).await {
                debug!("ERRR {:?} on {:?}", err, self.protocol);
            }
        }
    }
}


async fn mouse_server(mut receiever: mpsc::Receiver<MouseEvent>, return_sender: mpsc::Sender<MouseReturnEvent>, adapter: Arc<Adapter>) {
    let state = Arc::new(RwLock::new(<MouseState as Default>::default()));

    // Start advertising the keyboard functionality
    let advertisement_handle = adapter.advertise(Advertisement {
       advertisement_type: bluer::adv::Type::Peripheral,
       service_uuids: [hid::definitions::SERVICE].into(),
       discoverable: Some(true),
       appearance: Some(0x03c2),

       ..Default::default()
    }).await.expect("Error creating advertisement");
    
    // Create the GATT service
    let (mut boot_input_control, boot_input_handle) = characteristic_control();
    let (mut report_input_control, report_input_handle) = characteristic_control();
    let application_handle = adapter.serve_gatt_application(Application {
        services: vec![Service {
            uuid: hid::definitions::SERVICE,
            primary: true,
            characteristics: vec![
                hid::characteristics::protocol_mode(
                    callback!(|_request| state {
                        debug!("Read protocol by {}", _request.device_address);
                        Ok(vec![state.read().await.protocol.into()])
                    }),
                    callback!(|value, _request| state {
                        debug!("Write protocal by {} with {:?}", _request.device_address, value);
                        let protocol = match value.get(0) {
                            Some(0) => Some(Protocol::Boot),
                            Some(1) => Some(Protocol::Report),
                            _ => None
                        };
                        if let Some(protocol) = protocol {
                            state.write().await.protocol = protocol;
                        };
                        Ok(())
                    })
                ),
                hid::characteristics::information(super::hid::HID_INFORMATION),
                hid::characteristics::control_point(callback!(|value, _request| return_sender {
                    debug!("Write control point by {} with {:?}", _request.device_address, value);
                    return_sender.send(match value[0] {
                        0 => Ok(MouseReturnEvent::Suspend),
                        1 => Ok(MouseReturnEvent::Wake),
                        _ => Err(ReqError::Failed)
                    }?).await.unwrap();
                    Ok(())
                })),
                hid::characteristics::report_map(data::REPORT_DESCRIPTOR),
                hid::characteristics::boot_mouse_input(
                    callback!(|request| state {
                        let state = state.read().await;

                        debug!("Boot report  read by {}", request.device_address);
                        Ok(vec![state.buttons, 0, 0])
                    }),
                    boot_input_handle
                ),
                hid::characteristics::input_report(
                    callback!(|request| state {
                        let state = state.read().await;

                        debug!("Report read by {}", request.device_address);
                        Ok(vec![state.buttons, 0, 0])
                    }),
                    report_input_handle
                )
            ],
            ..Default::default()
        }],
        ..Default::default()
    }).await.expect("Failed to start GATT server");
    drop(adapter);


    enum Event {
        BootReportNotify(CharacteristicWriter),
        ReportNotify(CharacteristicWriter),
        MouseEvent(MouseEvent)
    }
    let mut events = async move || {
        tokio::select! {
            event = boot_input_control.next() => match event {
                Some(CharacteristicControlEvent::Notify(writer)) => Some(Event::BootReportNotify(writer)),
                _ => panic!("Invalid")
            },
            event = report_input_control.next() => match event {
                Some(CharacteristicControlEvent::Notify(writer)) => Some(Event::ReportNotify(writer)),
                _ => panic!("Invalid")
            },
            event = receiever.recv() => match event {
                Some(event) => Some(Event::MouseEvent(event)),
                None => None
            }
        }
    };

    while let Some(event) = events().await {
        let mut state = state.write().await;
        match event {
            Event::BootReportNotify(writer) => state.boot.push(writer),
            Event::ReportNotify(writer) => state.boot.push(writer),
            Event::MouseEvent(event) => match event {
                MouseEvent::ButtonPress(button) => {
                    // Subtract one from button ID to account for button 1 being at bit 0
                    if (1..=3).contains(&button.into_u16()) && state.buttons & (1<<(button.into_u16()-1)) == 0 {
                        state.buttons |= 1<<(button.into_u16()-1);
                        state.send_report(None).await;
                    }
                },
                MouseEvent::ButtonRelease(button) => {
                    // Subtract one from button ID to account for button 1 being at bit 0
                    if (1..=3).contains(&button.into_u16()) && state.buttons & (1<<(button.into_u16()-1)) != 0 {
                        state.buttons &= !(1<<(button.into_u16()-1));
                        state.send_report(None).await;
                    }
                    
                },
                MouseEvent::Movement(x, y) => state.send_report(Some((x, y))).await
            }
        }
    }

    drop(advertisement_handle);
    drop(application_handle);
}
