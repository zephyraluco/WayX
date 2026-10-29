//! Main-thread-only AppKit presentation; never activate or hide application windows.
use anyhow::{Context, Result, ensure};
use objc2::{ClassType, rc::Retained};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSImage};
use objc2_foundation::{MainThreadMarker, NSData};

fn application() -> Result<Retained<NSApplication>> {
    let marker = MainThreadMarker::new().context("AppKit presentation requires the main thread")?;
    Ok(NSApplication::sharedApplication(marker))
}

pub fn initialize(rootless: bool) -> Result<()> {
    let app = application()?;
    let data = NSData::with_bytes(include_bytes!("../assets/icons/wayx.icns"));
    let image =
        NSImage::initWithData(NSImage::alloc(), &data).context("Decode WayX application icon")?;
    // SAFETY: called on the main thread; AppKit retains the supplied image.
    unsafe { app.setApplicationIconImage(Some(&image)) };
    set_visible(!rootless)
}

pub fn set_visible(visible: bool) -> Result<()> {
    let app = application()?;
    let policy = if visible {
        NSApplicationActivationPolicy::Regular
    } else {
        NSApplicationActivationPolicy::Accessory
    };
    // SAFETY: NSApplication is accessed on the main thread.
    if unsafe { app.activationPolicy() } != policy {
        ensure!(
            app.setActivationPolicy(policy),
            "macOS rejected activation policy change"
        );
    }
    // SAFETY: NSApplication is accessed on the main thread.
    ensure!(
        unsafe { app.activationPolicy() } == policy,
        "macOS activation policy did not change"
    );
    tracing::info!(visible, "macOS application presentation");
    Ok(())
}
