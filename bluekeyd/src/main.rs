use std::sync::Arc;

// An actually half decent Bluetooth keyboard emulator
use evdev::{Device, EventSummary, InputEvent, KeyCode, RelativeAxisCode};
use tokio::io::{AsyncBufReadExt, BufReader};
use std::{time::{Duration, Instant}, path::PathBuf};
use clap::Parser;

use crate::bluetooth::{keyboard::{Keyboard, KeyboardReturnEvent, KeyboardServerDied}, mouse::{Button, Mouse, MouseServerDied}};

mod bluetooth;

#[derive(Debug)]
enum EvdevBridgeError {
    #[allow(unused)]
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

enum Never {}
async fn evdev_mouse_bridge(mouse: Mouse, device: Device) -> Result<Never, EvdevBridgeError> {
    let mut evdev_stream = device.into_event_stream()?;
    
    // Immeidately sending all mouse events caused horrific lag and queue backup
    let mut movement_clock = Clock::new(Duration::from_millis(8));
    let mut x = 0;
    let mut y = 0;
    let mut scroll = 0;

    #[derive(Debug)]
    enum Event {
        Evdev(EventSummary),
        MouseClock,
    }
    let mut next_event = async || {
        tokio::select! {
            event = evdev_stream.next_event() => match event {
                Ok(event) => Ok(Event::Evdev(event.destructure())),
                Err(error) => Err(EvdevBridgeError::EvdevError(error))
            },
            _ = movement_clock.next(std::time::Instant::now()) => Ok(Event::MouseClock),

        }
    };

    loop {
        let event = next_event().await?;
        //println!("{:?}",event);
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
            Event::Evdev(EventSummary::RelativeAxis(_, RelativeAxisCode::REL_WHEEL, amount)) => {
                scroll += amount;
            }
            Event::MouseClock => {
                let mx = x.clamp(i8::MIN as i32, i8::MAX as i32) as i8;
                let my = y.clamp(i8::MIN as i32, i8::MAX as i32) as i8;
                let ms = scroll.clamp(i8::MIN as i32, i8::MAX as i32) as i8;

                mouse.moved(mx, my, ms).await?;
                x -= mx as i32;
                y -= my as i32;
                scroll = 0;
            },

            _ => ()
        }
    }

}



#[derive(Parser)]
#[command(name = "bluekeyd")]
/// Pass a keyboard or mouse through an emulated Bluetooth device
///
/// Emulate a Bluetooth keyboard or mouse service from this computer,
/// forwarding a keyboard or mouse on this device through it.
/// Enables sharing a mouse or keyboard with another device via Bluetooth,
/// without the need of a special app or software.
struct Cli {
    #[clap(flatten)]
    devices: Devices,

    
    #[arg(long)]
    /// Skip the short wait before grabing the keyboard, to avoid a stuck enter key
    skip_wait: bool
}
#[derive(clap::Args)]
#[group(required = true)]
struct Devices {

    #[arg(long, short)]
    /// Path to keyboard device to forward
    keyboard: Option<PathBuf>,
    
    #[arg(long, short)]
    /// Path to mouse device to forward
    mouse: Option<PathBuf>,
}

struct Aborter {
    task: tokio::task::JoinHandle<Result<Never, EvdevBridgeError>>
}
impl Drop for Aborter {
    fn drop(&mut self) {
        self.task.abort(); // Summarily execute the task
    }
}

#[derive(Debug)]
struct Error(&'static str, Box<dyn std::fmt::Debug>);

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(Error(message, error)) = command().await {
        println!("{}", message);
        println!("Error: {:?}", error);
    }

}
async fn command() -> Result<(), Error> {
    let cli = Cli::parse();

    // A brief delay to avoid a stuck key(particularly enter) before grabbing keyboard
    if !cli.skip_wait {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    
    // Set up Bluetooth adapter and start evdev bridges
    let session = bluer::Session::new().await.map_err(|e| Error("Unable to open Bluetooth session(Is BlueZ running?)", Box::new(e)))?;
    let adapter = Arc::new(session.default_adapter().await.map_err(|e| Error("Unable to access Bluetooth adapter", Box::new(e)))?);

    let mouse = cli.devices.mouse.map(|device| {
        let mouse = Mouse::new(adapter.clone());
        let mut device = Device::open(device)?;
        device.grab()?;

        Ok(tokio::spawn(evdev_mouse_bridge(mouse, device)))
    }).transpose().map_err(|e: std::io::Error| Error("Error in Bluetooth mouse bridge", Box::new(e)))?.map(|task| Aborter { task });
    let keyboard = cli.devices.keyboard.map(|device| {
        let keyboard = Keyboard::new(adapter.clone());
        let mut device = Device::open(device)?;
        device.grab()?;

        Ok(tokio::spawn(evdev_keyboard_bridge(keyboard, device)))
    }).transpose().map_err(|e: std::io::Error| Error("Error in Bluetooth keyboard bridge", Box::new(e)))?;


    // If the keybaord is being used, exit via the keyboard(since you can't press enter if the keyboard is grabbed),
    // otherwise, wait for enter. 
    match keyboard {
        None => {
            let stdin = BufReader::new(tokio::io::stdin());
            let mut lines = stdin.lines();
            println!("Press enter to end forwarding");
            lines.next_line().await.map_err(|e| Error("Error getting stdin", Box::new(e)))?;
        },
        Some(keyboard) => {
            println!("Type 'super/windows + esc' to break keyboard grab");
            keyboard.await.expect("Panic in keyboard server").map_err(|e| Error("Error in keyboard bridge", Box::new(e)))?;
        }
    }
    
    drop(mouse);

    Ok(())
}