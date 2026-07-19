use std::sync::Arc;

// An actually half decent Bluetooth keyboard emulator
use evdev::{Device, EventSummary, InputEvent};
use tokio::io::AsyncBufReadExt;

use crate::bluetooth::{keyboard::{Keyboard, KeyboardReturnEvent, KeyboardServerDied}, mouse::{Button, mouse_server}};

mod bluetooth;

#[derive(Debug)]
enum EvdevBridgeError {
    EvdevError(std::io::Error),
    KeyboardError(KeyboardServerDied),
    UnmappedKey(u16)
}
impl From<std::io::Error> for EvdevBridgeError {
    fn from(value: std::io::Error) -> Self {
        EvdevBridgeError::EvdevError(value)
    }
}
impl From<KeyboardServerDied> for EvdevBridgeError {
    fn from(value: KeyboardServerDied) -> Self {
        EvdevBridgeError::KeyboardError(value)
    }
}

async fn evdev_bridge(mut keyboard: Keyboard, device: Device) -> Result<(), EvdevBridgeError> {
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

                    let map = keycode::KeyMap::from_key_mapping(keycode::KeyMapping::Evdev(code.0)).map_err(|()| EvdevBridgeError::UnmappedKey(code.0))?;
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


enum Errors {
    SessionAcquire(bluer::Error),
    AdapterAcquire(bluer::Error),
    DeviceOpen(std::io::Error),
    DeviceGrab(std::io::Error),
    BridgeError(EvdevBridgeError)
}
macro_rules! parse_error {
    ($error:literal) => {
        {
            println!($error);
            continue
        }
    };
}
macro_rules! parse_int {
    ($iter:ident->$var:ident as $T:ty $code:block) => {
        {
            use std::num::IntErrorKind;
            match $iter.next() {
                Some(number) => match number.parse::<$T>() {
                    Ok($var) => $code,
                    Err(error) if *error.kind() == IntErrorKind::Empty => parse_error!("Expected integer"),
                    Err(error) if *error.kind() == IntErrorKind::InvalidDigit => parse_error!("Invalid integer"),
                    Err(error) if *error.kind() == IntErrorKind::NegOverflow => parse_error!("Integer too low"),
                    Err(error) if *error.kind() == IntErrorKind::PosOverflow => parse_error!("Integer too high"),
                    Err(error) if *error.kind() == IntErrorKind::Zero => panic!("Invalid error"),
                    _ => parse_error!("Error parsing integer")
                },
                None => parse_error!("Expected argument")
            }
        }
    }
}


#[tokio::main(flavor = "current_thread")]
async fn main() {
    env_logger::init();

    let session = bluer::Session::new().await.unwrap();
    let adapter = Arc::new(session.default_adapter().await.unwrap());
    
    let mouse = bluetooth::mouse::Mouse::new(adapter.clone());
    let board = bluetooth::keyboard::start_keyboard(adapter.clone()).await;

    let stdin = tokio::io::BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    loop {
        let command = lines.next_line().await.unwrap().unwrap();
        let mut words = command.split(' ');
        
        match words.next() {
            Some("exit") => break,
            Some("up") => parse_int!(words->distance as i8 {
                mouse.moved(0, -distance).await.unwrap()
            }),
            Some("down") => parse_int!(words->distance as i8 {
                mouse.moved(0, distance).await.unwrap()
            }),
            Some("left") => parse_int!(words->distance as i8 {
                mouse.moved(-distance, 0).await.unwrap()
            }),
            Some("right") => parse_int!(words->distance as i8 {
                mouse.moved(distance, 0).await.unwrap()
            }),
            Some("press") => parse_int!(words->id as u16 {
                match id {
                    0 => parse_error!("0 is not a valid mouse button"),
                    _ => mouse.press(Button::from_id(1).unwrap()).await.unwrap()
                }        
            }),
            Some("release") => parse_int!(words->id as u16 {
                match id {
                    0 => parse_error!("0 is not a valid mouse button"),
                    _ => mouse.release(Button::from_id(1).unwrap()).await.unwrap()
                }                
            }),
            Some("kpress") => parse_int!(words->code as u8 {
                board.press(code).await.unwrap();
            }),
            Some("krelease") => parse_int!(words->code as u8 {
                board.release(code).await.unwrap();
            }),
            Some(_) => parse_error!("Invalid error"),
            None => continue

        }

    }

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
                EvdevBridgeError::KeyboardError(_) => println!("Bluetooth keyboard service died."),
                EvdevBridgeError::UnmappedKey(key) => println!("Unknown key(evdev id={}) received.", key)
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

    evdev_bridge(board, device).await.map_err(|e| Errors::BridgeError(e))?;
    
    Ok(())
}