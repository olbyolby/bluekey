use std::sync::Arc;

// An actually half decent Bluetooth keyboard emulator
use evdev::{Device, EventSummary, InputEvent, KeyCode, RelativeAxisCode};
use std::time::{Duration, Instant};

use crate::bluetooth::{keyboard::{Keyboard, KeyboardReturnEvent, KeyboardServerDied}, mouse::{Button, Mouse, MouseServerDied}};

mod bluetooth;

#[derive(Debug)]
enum EvdevBridgeError {
    EvdevError(std::io::Error),
    ServerDied
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
impl From<MouseServerDied> for EvdevBridgeError {
    fn from(_value: MouseServerDied) -> Self {
        EvdevBridgeError::ServerDied
    }
}

async fn evdev_keyboard_bridge(mut keyboard: Keyboard, device: Device) -> Result<(), EvdevBridgeError> {
    let mut evdev_stream = device.into_event_stream()?;

    let mut super_down = false;
    loop {
        tokio::select! {
            event = evdev_stream.next_event() => {
                if let EventSummary::Key(_, code, action) = event?.destructure() {
                    if code == evdev::KeyCode::KEY_LEFTMETA {
                        match action {
                            0 => super_down = false,
                            1 => super_down = true,
                            _ => ()
                        }
                    }
                    if code == evdev::KeyCode::KEY_ESC && action == 1 && super_down {
                        return Ok(())
                    }

                    let map = match keycode::KeyMap::from_key_mapping(keycode::KeyMapping::Evdev(code.0)) {
                        Ok(map) => map,
                        Err(_) => continue
                    };
                    let code: u8 = map.usb.try_into().expect("USB scancode is always 8 bits");
                    match action {
                        0 => keyboard.release(code).await?,
                        1 => keyboard.press(code).await?,
                        _ => ()
                    };
                    
                }
            },
            event = keyboard.next_event() => {
                match event? {
                    KeyboardReturnEvent::LedOn(led) => set_led(evdev_stream.device_mut(), led, true)?,
                    KeyboardReturnEvent::LedOff(led) => set_led(evdev_stream.device_mut(), led, false)?,
                    _ => ()
                }
            }
        }
    }
}
fn set_led(device: &mut Device, led: bluetooth::leds::Led, on: bool) -> Result<(), std::io::Error> {
    let on = match on {
        true => 1,
        false => 0
    };

    device.send_events(&[
        InputEvent::new(evdev::EventType::LED.0, led.into_id().into(), on)
    ])
}


fn map_button_codes(button: KeyCode) -> Option<Button> {
    match button {
        KeyCode::BTN_LEFT => Some(Button::from_id(1).unwrap()),
        KeyCode::BTN_RIGHT => Some(Button::from_id(2).unwrap()),
        KeyCode::BTN_MIDDLE => Some(Button::from_id(3).unwrap()),
        _ => None
    }
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


async fn evdev_mouse_bridge(mouse: Mouse, device: Device) -> Result<(), EvdevBridgeError> {
    let mut evdev_stream = device.into_event_stream()?;
    


    let mut movement_clock = Clock::new(Duration::from_millis(8));
    let mut x = 0;
    let mut y = 0;

    enum Event {
        Evdev(EventSummary),
        MouseClock,
    }
    let mut next_event = async || {
        tokio::select! {
            event = evdev_stream.next_event() => match event {
                Ok(event) => Ok(Event::Evdev(event.destructure())),
                Err(_) => Err(())
            },
            _ = movement_clock.next(std::time::Instant::now()) => Ok(Event::MouseClock),

        }
    };

    while let Ok(event) = next_event().await {
        match event {
            Event::Evdev(EventSummary::Key(_, code, action)) if let Some(button) = map_button_codes(code) => match action {
                0 => mouse.release(button).await?,
                1 => mouse.press(button).await?,
                _ => ()
            },
            Event::Evdev(EventSummary::RelativeAxis(_, RelativeAxisCode::REL_X, amount)) => {
                x += amount;
            },
            Event::Evdev(EventSummary::RelativeAxis(_, RelativeAxisCode::REL_Y, amount)) => {
                y += amount;
            },
            Event::MouseClock => {
                let mx = x.clamp(i8::MIN as i32, i8::MAX as i32) as i8;
                let my = y.clamp(i8::MIN as i32, i8::MAX as i32) as i8;

                mouse.moved(mx, my).await.unwrap();
                x -= mx as i32;
                y -= my as i32;
            },
            _ => ()
        }
    }

    Ok(())
}




enum Errors {
    SessionAcquire(bluer::Error),
    AdapterAcquire(bluer::Error),
    DeviceOpen(std::io::Error),
    DeviceGrab(std::io::Error),
    BridgeError(EvdevBridgeError)
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    env_logger::init();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let session = bluer::Session::new().await.unwrap();
    let adapter = Arc::new(session.default_adapter().await.unwrap());
    
    let mouse = bluetooth::mouse::Mouse::new(adapter.clone());
    let board = bluetooth::keyboard::start_keyboard(adapter.clone()).await;

    let mut mouse_device = evdev::Device::open("/dev/input/event23").unwrap();
    let mut keyboard_device = evdev::Device::open("/dev/input/by-id/usb-Razer_Razer_Ornata_Chroma-event-kbd").unwrap();

    mouse_device.grab().unwrap();
    keyboard_device.grab().unwrap();

    let mouse_bridge = tokio::spawn(evdev_mouse_bridge(mouse, mouse_device));
    evdev_keyboard_bridge(board, keyboard_device).await.unwrap();
    mouse_bridge.abort();
}




#[allow(dead_code)]
async fn main2() {
    let mut args = std::env::args();

    args.next();
    let device = match args.next() {
        Some(device) => device,
        None => return println!("No device or option provided")
    };
    if let Some(_) = args.next() {
        return println!("Too many arguments supplied");
    }
    
    if device == "-h" {
        println!("bluekeyd [-h] device_path");
        return 
    }

    // If grabing an in-use device, grab can happen before enter is released, leaving it stuck to the OS, so give a delay for that
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    println!("Type 'super/windows + esc' to break keyboard grab");
    if let Err(error) = start(&device).await {
        match error {
            Errors::AdapterAcquire(e) => println!("Error estbalishing bluetooth connection.\nMessage: {}", e.message),
            Errors::SessionAcquire(e) => println!("Error accessing bluetooth adapter.\nMessage: {}", e.message),
            Errors::DeviceOpen(e) => println!("Error opening keyboard device(do you have permission?)\n{}", e),
            Errors::DeviceGrab(e) => println!("Error grabing keyboard device(is it already grabbed?)\n{}", e),
            Errors::BridgeError(e) => match e {
                EvdevBridgeError::EvdevError(e) => println!("Error with evdev device.\n{}", e),
                EvdevBridgeError::ServerDied => println!("Bluetooth keyboard service died."),
            }
        }
    }
}

async fn start(device: &str) -> Result<(), Errors>{
    let session = bluer::Session::new().await.map_err(|e| Errors::SessionAcquire(e))?;
    let adapter = Arc::new(session.default_adapter().await.map_err(|e| Errors::AdapterAcquire(e))?);

    let mut device = Device::open(device).map_err(|e| Errors::DeviceOpen(e))?;
    device.grab().map_err(|e| Errors::DeviceGrab(e))?;
    
    let board = bluetooth::keyboard::start_keyboard(adapter).await;

    evdev_keyboard_bridge(board, device).await.map_err(|e| Errors::BridgeError(e))?;
    
    Ok(())
}