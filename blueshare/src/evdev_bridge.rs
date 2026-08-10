use evdev::{Device, EventStream, EventSummary, InputEvent, KeyCode, RelativeAxisCode};
use std::{borrow::Borrow, sync::Arc, time::{Duration, Instant}};
use tokio::{sync::oneshot, task::JoinHandle};

use crate::bluetooth::{ReturnError, Target, keyboard::{Keyboard, KeyboardReturnEvent, KeyboardServerDied}, leds::Led, mouse::{Button, Mouse, MouseServerDied}};


#[derive(Debug)]
pub enum EvdevBridgeError {
    #[allow(unused)]
    EvdevError(std::io::Error),
    ServerDied,
    Desynced
}
impl From<std::io::Error> for EvdevBridgeError {
    fn from(value: std::io::Error) -> Self {
        EvdevBridgeError::EvdevError(value)
    }
}
impl From<KeyboardServerDied> for EvdevBridgeError {
    fn from(_value: KeyboardServerDied) -> Self {
        EvdevBridgeError::ServerDied
    }
}
impl From<ReturnError> for EvdevBridgeError {
    fn from(value: ReturnError) -> Self {
        match value {
            ReturnError::Lagged(_) => EvdevBridgeError::Desynced,
            ReturnError::ServerDied => EvdevBridgeError::ServerDied
        }
    }
}


impl From<MouseServerDied> for EvdevBridgeError {
    fn from(_value: MouseServerDied) -> Self {
        EvdevBridgeError::ServerDied
    }
}

#[derive(Debug, Clone)]
pub struct Shortcut {
    shortcut: Arc<Vec<KeyCode>>,
    progress: usize
}
impl Shortcut {
    pub fn new(shortcut: Arc<Vec<KeyCode>>) -> Self {
        Self {
            shortcut,
            progress: 0
        }
    }
    fn pressed(&mut self, keycode: KeyCode) -> bool {
        if self.shortcut.contains(&keycode) {
            self.progress += 1;

            if self.progress == self.shortcut.len() {
                self.progress = 0;
                return true
            } 
            return false
        } 
        return false
    }
    fn released(&mut self, keycode: KeyCode) {
        if self.shortcut.contains(&keycode) {
            self.progress = 0;
        }
    }
}



struct Cancel;
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BreakReason {
    Cancelled,
    Escaped
}


pub struct KeyboardBridge {
    canceller: Option<oneshot::Sender<Cancel>>,
    handle: JoinHandle<Result<BreakReason, EvdevBridgeError>>
}
impl KeyboardBridge {
    pub fn start<T: Borrow<Keyboard> + Send + 'static, F: FnOnce() -> R + Send + 'static, R: Future<Output=()> + Send>(keyboard: T, device: EventStream, target: Target, escape: Shortcut, notify: F) -> Self {
        let (canceller, cancelee) = oneshot::channel();
        let handle = tokio::spawn(async move {
            let result = evdev_keyboard_bridge(keyboard, device, target, cancelee, escape).await;
            if !matches!(result, Ok(BreakReason::Cancelled)) {
                notify().await;
            }
            result
        });        
        
        Self {
            handle,
            canceller: Some(canceller)
        }
    }
    pub async fn cancel(mut self) -> Result<BreakReason, EvdevBridgeError> {
        if let Some(canceller) = self.canceller.take() {
            let _ = canceller.send(Cancel); // If the channel is closed, the bridge has stoped and handle has a value
        }

        (&mut self.handle).await.unwrap() // Propgate panics
    }
    pub async fn wait_for_break(mut self) -> Result<BreakReason, EvdevBridgeError> {
        (&mut self.handle).await.unwrap()
    }
}
impl Drop for KeyboardBridge {
    fn drop(&mut self) {
        if let Some(canceller) = self.canceller.take() {
            let _ = canceller.send(Cancel); // If the channel is closed, the bridge has stoped and handle has a value
        }
    }
}


async fn evdev_keyboard_bridge<T: Borrow<Keyboard> + Send + 'static>(keyboard: T, mut device: EventStream, target: Target, mut cancel: oneshot::Receiver<Cancel>, mut escape: Shortcut) -> Result<BreakReason, EvdevBridgeError> { 
    let keyboard = keyboard.borrow();

    let mut returns = match target {
        Target::Target(target) => Some(target),
        Target::Broadcast => None
    };

    let mut keyboard_events = keyboard.listen();

    loop {
        tokio::select! {
            event = device.next_event() => {
                if let EventSummary::Key(_, code, action) = event?.destructure() {
                    match action {
                        0 => escape.released(code),
                        1 => if escape.pressed(code) {
                            for key in escape.shortcut.iter() {
                                if let Ok(code) = keycode::KeyMap::try_from(keycode::KeyMapping::Evdev(key.0)) {
                                    keyboard.release(target, code.usb as u8).await?
                                }
                            }
                            break Ok(BreakReason::Escaped)
                        },
                        _ => ()
                    }

                    let map = match keycode::KeyMap::from_key_mapping(keycode::KeyMapping::Evdev(code.0)) {
                        Ok(map) => map,
                        Err(_) => continue
                    };
                    let code: u8 = map.usb.try_into().expect("USB scancode is always 8 bits");
                    match action {
                        0 => keyboard.release(target,code).await?,
                        1 => keyboard.press(target, code).await?,
                        _ => ()
                    };
                    
                }
            },
            event = keyboard_events.next_event() => if let Some(target) = returns {
                match event? {
                    KeyboardReturnEvent::LedOn(from, led)  if from==target => set_led(device.device_mut(), led, true)?,
                    KeyboardReturnEvent::LedOff(from, led) if from==target => set_led(device.device_mut(), led, false)?,
                    KeyboardReturnEvent::Register(address) if returns == None => returns = Some(address),
                    _ => ()
                }                
            },
            _ = &mut cancel => return Ok(BreakReason::Cancelled)
        }
    }
}
fn set_led(device: &mut Device, led: Led, on: bool) -> Result<(), std::io::Error> {
    let on = match on {
        true => 1,
        false => 0
    };

    device.send_events(&[
        InputEvent::new(evdev::EventType::LED.0, led.into_id().into(), on)
    ])
}

struct Clock {
    duration: Duration,
    last: Instant
}
impl Clock {
    fn new(duration: Duration) -> Self {
        Clock {
            duration,
            last: Instant::now()
        }
    }
    async fn next(&mut self, instant: Instant) {
        let remaining = self.duration.saturating_sub(instant.saturating_duration_since(self.last));
        
        if remaining > Duration::new(0, 0) {
            tokio::time::sleep(remaining).await;
        }
        self.last = Instant::now();
    }
}

fn map_button_codes(button: KeyCode) -> Option<Button> {
    match button {
        KeyCode::BTN_LEFT => Some(Button::from_id(1).unwrap()),
        KeyCode::BTN_RIGHT => Some(Button::from_id(2).unwrap()),
        KeyCode::BTN_MIDDLE => Some(Button::from_id(3).unwrap()),
        _ => None
    }
}

pub struct MouseBridge {
    canceller: Option<oneshot::Sender<Cancel>>,
    handle: JoinHandle<Result<(), EvdevBridgeError>>
}
impl MouseBridge {
    pub fn start<T: Borrow<Mouse> + Send + 'static>(mouse: T, device: EventStream, target: Target) -> Self {
        let (canceller, cancelee) = oneshot::channel();
        let handle = tokio::spawn(evdev_mouse_bridge(mouse, device, target, cancelee));
        Self {
            handle,
            canceller: Some(canceller)
        }
    }
    pub async fn cancel(mut self) -> Result<(), EvdevBridgeError> {
        if let Some(canceller) = self.canceller.take() {
            let _ = canceller.send(Cancel); // If the channel is closed, the bridge has stoped and handle has a value
        }
        (&mut self.handle).await.unwrap() // Propgate panics
    }
    pub async fn wait_for_break(mut self) -> Result<(), EvdevBridgeError> {
        (&mut self.handle).await.unwrap()
    }
}
impl Drop for MouseBridge {
    fn drop(&mut self) {
        if let Some(canceller) = self.canceller.take() {
            let _ = canceller.send(Cancel); // If the channel is closed, the bridge has stoped and handle has a value
        }
    }
}


async fn evdev_mouse_bridge<T: Borrow<Mouse> + Send + 'static>(mouse: T, mut device: EventStream, target: Target, mut canceller: oneshot::Receiver<Cancel>) -> Result<(), EvdevBridgeError> {  
    let mouse = mouse.borrow();

    // Immeidately sending all mouse events caused horrific lag and queue backup
    // The optimal time here depends on the connection interval, which, as far as I know, BlueZ provides no easy way to find
    // Any faster than the connection interval, and reports start backing up. This *could* be improved using a Bluetooth Classic device,
    // but that is a *major* rewrite. 
    let mut movement_clock = Clock::new(Duration::from_millis(30));
    let mut log_clock = Clock::new(Duration::from_millis(1000));
    let mut x = 0;
    let mut y = 0;
    let mut scroll = 0;

    #[derive(Debug)]
    enum Event {
        Evdev(EventSummary),
        MouseClock,
        LogClock,
        Cancel
    }
    let mut next_event = async || {
        tokio::select! {
            event = device.next_event() => match event {
                Ok(event) => Ok(Event::Evdev(event.destructure())),
                Err(error) => Err(EvdevBridgeError::EvdevError(error))
            },
            _ = movement_clock.next(std::time::Instant::now()) => Ok(Event::MouseClock),
            _ = log_clock.next(std::time::Instant::now()) => Ok(Event::LogClock),
            _ = &mut canceller => Ok(Event::Cancel)
        }
    };

    loop {
        let event = next_event().await?;
        match event {
            Event::Evdev(EventSummary::Key(_, code, action)) if let Some(button) = map_button_codes(code) => match action {
                0 => mouse.release(target, button).await?,
                1 => mouse.press(target, button).await?,
                _ => ()
            },
            Event::Evdev(EventSummary::RelativeAxis(_, RelativeAxisCode::REL_X, amount)) => {
                x += amount;
            },
            Event::Evdev(EventSummary::RelativeAxis(_, RelativeAxisCode::REL_Y, amount)) => {
                y += amount;
            },
            Event::Evdev(EventSummary::RelativeAxis(_, RelativeAxisCode::REL_WHEEL, amount)) => {
                scroll += amount;
            },
            Event::Evdev(_) => (),

            Event::MouseClock => {
                let mx = x.clamp(i8::MIN as i32, i8::MAX as i32) as i8;
                let my = y.clamp(i8::MIN as i32, i8::MAX as i32) as i8;
                let ms = scroll.clamp(i8::MIN as i32, i8::MAX as i32) as i8;

                mouse.moved(target, mx, my, ms).await?;
                x -= mx as i32;
                y -= my as i32;
                scroll = 0;
            },
            Event::LogClock => {
                //println!("Avg delta: {}ms, behind: {}ms", delta_sum.as_millis()/(events as u128).max(1), std::time::SystemTime::now().duration_since(latest).unwrap_or(Duration::new(0,0)).as_millis())
            },
            Event::Cancel => return Ok(())

            
        }
    }

}
