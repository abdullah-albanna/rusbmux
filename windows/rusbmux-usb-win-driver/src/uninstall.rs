use std::process::Command;

#[derive(Debug)]
pub struct PnpEntity {
    inf_name: String,
}

fn remove_device(inf_name: &str) -> Result<(), i32> {
    let output = match Command::new("pnputil")
        .args(["/delete-driver", inf_name, "/uninstall"])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            eprintln!("Failed to delete device: {e:?}");
            return Err(677);
        }
    };

    if !output.stderr.is_empty() {
        eprintln!(
            "Failed to delete device: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return Err(677);
    }

    Ok(())
}

fn rescan() -> Result<(), i32> {
    if !Command::new("pnputil")
        .args(["/scan-devices"])
        .status()
        .map_err(|_| 667)?
        .success()
    {
        return Err(667);
    }

    Ok(())
}

/// Parse `pnputil /enum-devices` output to find Apple devices with non-Microsoft drivers.
///
/// Some entries (like WPD Portable Devices) lack an `Instance ID:` line entirely,
/// so we parse by blank-line-delimited blocks instead of relying on a specific first field.
fn find_apple_devices() -> Result<Vec<PnpEntity>, i32> {
    let output = match Command::new("pnputil").args(["/enum-devices"]).output() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("Failed to run pnputil /enum-devices: {e}");
            return Err(67);
        }
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let mut entities = Vec::new();
    let mut lines = text.lines().peekable();

    loop {
        // skip leading blank lines
        while let Some(l) = lines.peek() {
            if l.trim().is_empty() {
                lines.next();
            } else {
                break;
            }
        }

        if lines.peek().is_none() {
            break;
        }

        // collect one device block (instance id is optional; some WPD entries omit it)
        let mut instance_id = None;
        let mut device_desc = None;
        let mut manufacturer = None;
        let mut driver_name = None;

        loop {
            match lines.peek() {
                Some(l) if l.trim().is_empty() => {
                    let _ = lines.next();
                    break;
                }
                Some(_l) => {
                    let raw = lines.next().unwrap();
                    let trimmed = raw.trim();

                    if let Some(v) = trimmed.strip_prefix("Instance ID:") {
                        instance_id = Some(v.trim().to_string());
                    } else if let Some(v) = trimmed.strip_prefix("Device Description:") {
                        device_desc = Some(v.trim().to_string());
                    } else if let Some(v) = trimmed.strip_prefix("Manufacturer Name:") {
                        manufacturer = Some(v.trim().to_string());
                    } else if let Some(v) = trimmed.strip_prefix("Driver Name:") {
                        driver_name = Some(v.trim().to_string());
                    }
                }
                None => break,
            }
        }

        let is_apple = instance_id
            .as_deref()
            .is_some_and(|id| id.contains("VID_05AC"))
            || device_desc.as_deref().is_some_and(|d| {
                d.contains("iPhone")
                    || d.contains("iPad")
                    || d.contains("iPod")
                    || d.contains("Apple")
            })
            || manufacturer.as_deref().is_some_and(|m| m.contains("Apple"));

        if let (Some(inf), Some(prov)) = (&driver_name, &manufacturer) {
            // we don't want to remove the default drivers
            if is_apple && (!prov.contains("Microsoft") && !inf.contains("wpdmtp")) {
                entities.push(PnpEntity {
                    inf_name: inf.clone(),
                });
            }
        }
    }

    dbg!(&entities);

    Ok(entities)
}

pub fn uninstall_any_apple_driver(rescans: u8) -> Result<(), i32> {
    for _ in 0..rescans {
        let devices = find_apple_devices()?;
        if devices.is_empty() {
            break;
        }
        for dev in &devices {
            remove_device(&dev.inf_name)?;
        }
        rescan()?;
    }

    Ok(())
}
