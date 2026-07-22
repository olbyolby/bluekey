#![allow(dead_code)] // Annoying 'cause I have unfinished APIs
use std::{str::FromStr, sync::Arc};

use bluer::Address;
// An actually half decent Bluetooth keyboard emulator
use evdev::{Device, EventSummary, InputEvent, KeyCode, RelativeAxisCode};
use tokio::io::{AsyncBufReadExt, BufReader};
use std::{time::{Duration, Instant}, path::PathBuf};
use clap::Parser;

use crate::bluetooth::{Target, keyboard::{Keyboard, KeyboardReturnEvent, KeyboardServerDied}, mouse::{Button, Mouse, MouseServerDied}};

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


async fn evdev_keyboard_bridge(mut keyboard: Keyboard, device: Device, target: Target) -> Result<(), EvdevBridgeError> {
    let mut evdev_stream = device.into_event_stream()?;
    
    let mut super_down = false;
    let mut returns = match target {
        Target::Target(target) => Some(target),
        Target::Broadcast => None
    };

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
                        // Avoid stuck super/windows key
                        keyboard.release(target, keycode::KeyMap::try_from(keycode::KeyMapping::Evdev(evdev::KeyCode::KEY_LEFTMETA.0)).unwrap().usb as u8).await.unwrap();
                        return Ok(())
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
            event = keyboard.next_event() => if let Some(target) = returns {
                match event? {
                    KeyboardReturnEvent::LedOn(from, led)  if from==target => set_led(evdev_stream.device_mut(), led, true)?,
                    KeyboardReturnEvent::LedOff(from, led) if from==target => set_led(evdev_stream.device_mut(), led, false)?,
                    KeyboardReturnEvent::Register(address) if returns == None => returns = Some(address),
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

async fn evdev_mouse_bridge(mouse: Mouse, device: Device, target: Target) -> Result<std::convert::Infallible, EvdevBridgeError> {
    let mut evdev_stream = device.into_event_stream()?;
    
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
        LogClock
    }
    let mut next_event = async || {
        tokio::select! {
            event = evdev_stream.next_event() => match event {
                Ok(event) => Ok(Event::Evdev(event.destructure())),
                Err(error) => Err(EvdevBridgeError::EvdevError(error))
            },
            _ = movement_clock.next(std::time::Instant::now()) => Ok(Event::MouseClock),
            _ = log_clock.next(std::time::Instant::now()) => Ok(Event::LogClock)
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
            }
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
            }

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
/// 
/// Note: You may need to run this program with the input group to access
/// evdev devices. An easy way to temporarily open a hash with this group is:
/// `sudo --preserve-env setpriv --regid $(id -g $USER) --reuid $(id -u $USER) --groups input,$(id -G $USER | sed "s/ /,/g") bash`
struct Cli {
    #[clap(flatten)]
    devices: Devices,

    #[arg(long)]
    /// Mac address of device to connect
    mac: Option<String>,

    #[arg(long)]
    /// Skip the short delay before grabing the keyboard(delay is to avoid a stuck enter key)
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
    task: tokio::task::JoinHandle<Result<std::convert::Infallible, EvdevBridgeError>>
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
    env_logger::init();
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

    // Get address
    let address = cli.mac
        .map(|mac| Address::from_str(&mac)
            .map(|a| Target::Target(a))
            .map_err(|e| Error("Invalid Mac address formatting", Box::new(e))))
        .transpose()?.unwrap_or_else(|| {
            println!("Warning: With no MAC address specified, the server will use Broadcast Mode and ALL connected devices will recieve input simultaniously.");
            println!("Additionally, the keyboard will only display LEDs(Caps lock, num lock) for the first connected device.");
            Target::Broadcast
        });
        
    // Set up Bluetooth adapter and start evdev bridges
    let session = bluer::Session::new().await.map_err(|e| Error("Unable to open Bluetooth session(Is BlueZ running?)", Box::new(e)))?;
    let adapter = Arc::new(session.default_adapter().await.map_err(|e| Error("Unable to access Bluetooth adapter", Box::new(e)))?);

    let mouse = cli.devices.mouse.map(|device| {
        let mouse = Mouse::new(adapter.clone());
        let mut device = Device::open(device)?;
        device.grab()?;

        Ok(tokio::spawn(evdev_mouse_bridge(mouse, device, address)))
    }).transpose().map_err(|e: std::io::Error| Error("Error establishing Bluetooth mouse bridge", Box::new(e)))?.map(|task| Aborter { task });
    let keyboard = cli.devices.keyboard.map(|device| {
        let keyboard = Keyboard::new(adapter.clone());
        let mut device = Device::open(device)?;
        device.grab()?;

        Ok(tokio::spawn(evdev_keyboard_bridge(keyboard, device, address)))
    }).transpose().map_err(|e: std::io::Error| Error("Error establishing Bluetooth keyboard bridge", Box::new(e)))?;

    // If the keyboard is being used, exit via the keyboard(since you can't press enter if the keyboard is grabbed),
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