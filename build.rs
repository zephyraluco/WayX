use std::{env, path::PathBuf, process::Command};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=assets/icons/wayx.rc");
    println!("cargo:rerun-if-changed=assets/icons/wayx.ico");
    println!("cargo:rerun-if-env-changed=RC");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || env::var_os("CARGO_FEATURE_DESKTOP").is_none()
    {
        return Ok(());
    }
    let directory =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("Missing manifest directory")?)
            .join("assets/icons");
    let output = PathBuf::from(env::var_os("OUT_DIR").ok_or("Missing build output directory")?);
    let msvc = env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    let resource = output.join(if msvc { "wayx.res" } else { "wayx-resource.o" });
    let compiler = env::var_os("RC").unwrap_or_else(|| {
        if msvc {
            "rc.exe".into()
        } else if env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("x86") {
            "i686-w64-mingw32-windres".into()
        } else {
            "x86_64-w64-mingw32-windres".into()
        }
    });
    let mut command = Command::new(compiler);
    command.current_dir(directory);
    if msvc {
        command
            .arg("/nologo")
            .arg("/fo")
            .arg(&resource)
            .arg("wayx.rc");
    } else {
        command.arg("-i").arg("wayx.rc").arg("-o").arg(&resource);
    }
    let status = command.status().map_err(|error| {
        format!("Run Windows SDK rc.exe or MinGW windres (override with RC): {error}")
    })?;
    if !status.success() {
        return Err("Windows icon resource compilation failed".into());
    }
    println!("cargo:rustc-link-arg-bin=wayx={}", resource.display());
    Ok(())
}
