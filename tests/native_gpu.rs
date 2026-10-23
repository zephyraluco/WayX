//! Interactive X11 windows driven by the native GPU lifetime regression harness.
#[path = "support/x11.rs"]
mod x11;
use anyhow::{Result, ensure};
use std::{
    io::{BufRead, Write},
    time::{Duration, Instant},
};
use x11rb::{connection::Connection, protocol::xproto::*, wrapper::ConnectionExt as _};
#[test]
#[ignore = "driven by scripts/test-idle-cpu.py --gpu-cycles against a native server"]
fn native_gpu_client() -> Result<()> {
    let conn = x11::connect_native()?;
    let root = conn.setup().roots[0].root;
    let mut windows = Vec::new();
    println!("GPU READY");
    std::io::stdout().flush()?;
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        let args: Vec<_> = line.split_whitespace().collect();
        match args.as_slice() {
            ["open", width, height, count] => {
                ensure!(windows.is_empty(), "Previous windows are still open");
                let (width, height, count): (u16, u16, u16) =
                    (width.parse()?, height.parse()?, count.parse()?);
                for index in 0..count {
                    let window = conn.generate_id()?;
                    conn.create_window(
                        0,
                        window,
                        root,
                        20 + index as i16 * 30,
                        20 + index as i16 * 30,
                        width,
                        height,
                        0,
                        WindowClass::INPUT_OUTPUT,
                        0,
                        &CreateWindowAux::new(),
                    )?
                    .check()?;
                    conn.change_property8(
                        PropMode::REPLACE,
                        window,
                        AtomEnum::WM_NAME,
                        AtomEnum::STRING,
                        format!("WayX GPU lifetime {index}").as_bytes(),
                    )?;
                    conn.map_window(window)?.check()?;
                    let gc = conn.generate_id()?;
                    conn.create_gc(
                        gc,
                        window,
                        &CreateGCAux::new().foreground(if index == 0 {
                            0xff0000
                        } else {
                            0x0000ff
                        }),
                    )?;
                    conn.poly_fill_rectangle(
                        window,
                        gc,
                        &[Rectangle {
                            x: 0,
                            y: 0,
                            width,
                            height,
                        }],
                    )?;
                    conn.free_gc(gc)?;
                    windows.push(window);
                }
                conn.flush()?;
                println!(
                    "GPU OPEN {}",
                    windows
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                );
            }
            ["resize", width, height] => {
                let (width, height): (u16, u16) = (width.parse()?, height.parse()?);
                let window = *windows
                    .first()
                    .ok_or_else(|| anyhow::anyhow!("No live window"))?;
                conn.configure_window(
                    window,
                    &ConfigureWindowAux::new()
                        .width(u32::from(width))
                        .height(u32::from(height)),
                )?
                .check()?;
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    let geometry = conn.get_geometry(window)?.reply()?;
                    if (geometry.width, geometry.height) == (width, height) {
                        break;
                    }
                    ensure!(Instant::now() < deadline, "Resize did not converge");
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            ["close-one"] => {
                if let Some(window) = windows.pop() {
                    conn.destroy_window(window)?.check()?;
                }
            }
            ["close"] => {
                for window in windows.drain(..) {
                    conn.destroy_window(window)?.check()?;
                }
            }
            ["quit"] => break,
            _ => anyhow::bail!("Unknown GPU command: {line}"),
        }
        conn.flush()?;
        println!("GPU DONE");
        std::io::stdout().flush()?;
    }
    for window in windows {
        conn.destroy_window(window)?.check()?;
    }
    Ok(())
}
