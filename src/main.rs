// A very awful, but technically functional, Bluetooth keyboard emulator
// Now with the extremely jank capability to switch between devices!
use std::sync::{Arc, Condvar, atomic::AtomicBool, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};
use evdev::{Device, EventSummary, KeyCode};

mod bluetooth;

#[derive(Clone, Copy, PartialEq, Eq)]
enum States {
    Stop,
    Run,
    Wait
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> bluer::Result<()> {
    //main2().await;

    println!("Creating keyboard interface");
    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    let board = bluetooth::keyboard::start_keyboard(adapter).await?;

    println!("Press enter to quit");
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    
    println!("Pulling events");
    let mut macros = Device::open("/dev/input/by-id/usb-_RPI_Wired_Keyboard_4-event-kbd").unwrap();
    macros.grab().unwrap();
    
    let pair = Arc::new((Condvar::new(), Mutex::new(States::Run)));
    let pair2 = Arc::clone(&pair);
    let pair3 = Arc::clone(&pair);

    std::thread::spawn(move || {
        let (condvar, mutex) = &*pair2;

        let mut lock = mutex.lock().unwrap();
        let mut device = Device::open("/dev/input/by-id/usb-Razer_Razer_Ornata_Chroma-event-kbd").unwrap();
        device.grab().unwrap();

        while *lock!=States::Stop {
            if *lock==States::Run {
                drop(lock);
                for event in device.fetch_events().unwrap() {
                    if let EventSummary::Key(_, code, action) = event.destructure() {
                        if let Ok(map) = keycode::KeyMap::from_key_mapping(keycode::KeyMapping::Evdev(code.0)) {
                            let code: u8 = map.usb.try_into().unwrap();
                            match action {
                                0 => board.try_release(code).unwrap(),
                                1 => board.try_press(code).unwrap(),
                                _ => println!("Invalid action: {:?}", action)
                            };
                        } else {
                            println!("Invalid key: {:?}", code);
                        }
                    }
                    
                }
                lock = mutex.lock().unwrap();
            } else {
                device.ungrab().unwrap();
                lock = condvar.wait(lock).unwrap();
                if *lock == States::Run {
                    device = Device::open("/dev/input/by-id/usb-Razer_Razer_Ornata_Chroma-event-kbd").unwrap();
                    device.grab().unwrap();
                }
            }


            
        }
    });
    std::thread::spawn(move || {
        let (condvar, mutex) = &*pair3;
        while *mutex.lock().unwrap() != States::Stop {
            for event in macros.fetch_events().unwrap() {
                if let EventSummary::Key(_, code, 1) = event.destructure() {
                    match code {
                        KeyCode::KEY_ESC => {
                            println!("Stopping");
                            *mutex.lock().unwrap() = States::Stop;
                            condvar.notify_all();
                        },
                        KeyCode::KEY_DELETE => {
                            std::process::abort(); // Nuclear options
                        }
                        KeyCode::KEY_1 => {
                            let mut lock = mutex.lock().unwrap();
                            if *lock == States::Wait {
                                println!("Starting events");
                                *lock = States::Run;
                                condvar.notify_all();
                            } else if *lock == States::Run {
                                println!("Stopping events");
                                *lock = States::Wait;
                                condvar.notify_all();
                            }
                        }
                        _ => ()
                    }
                }
            }
        }

    });

    loop {
        tokio::select! {
            _ = lines.next_line() => break 
            
        }
    }

    let (condvar, mutex) = &*pair;
    *mutex.lock().unwrap() = States::Stop;
    condvar.notify_all();

    Ok(())

    
}
