// amVideo-rs
// Copyright (C) 2020  Matt Bilker <me@mbilker.us>
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

#[macro_use(anyhow)]
extern crate anyhow;
#[macro_use(const_assert_eq)]
extern crate static_assertions;

use std::error::Error as StdError;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::Error;
use std::mem;
use std::os::windows::ffi::OsStrExt;
use std::str;

use anyhow::{Context, Result};
use winapi::um::libloaderapi::LoadLibraryW;
use winreg::enums::HKEY_LOCAL_MACHINE;
use winreg::RegKey;

mod library_handle;

use crate::library_handle::LibraryHandle;

const AM_VIDEO_CONTEXT_DATA_SIZE: usize = 0x400 - mem::size_of::<u32>();

#[repr(C)]
struct AmVideoContext {
    version: u32,
    data: [u8; AM_VIDEO_CONTEXT_DATA_SIZE],
}

#[derive(Debug)]
#[repr(C)]
struct AmVideoSetting {
    version: u32,
    use_segatiming: u32,
    mode: AmVideoMode,
    resolution_1: AmVideoResolution,
    resolution_2: AmVideoResolution,
}

#[allow(unused)]
#[derive(Debug)]
#[repr(u32)]
enum AmVideoMode {
    /// Single display mode using `resolution_1`
    Single = 0,
    /// Single or dual display mode using `resolution_1` for both displays. Does not fail if a
    /// second display is not connected.
    CloneVideoMode = 1,
    /// Dual display mode using both `resolution_1` and `resolution_2`
    DualVideoMode = 4,
}

#[derive(Debug)]
#[repr(C)]
struct AmVideoResolution {
    width: u16,
    height: u16,
}

impl Default for AmVideoResolution {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
        }
    }
}

// Ensure structure sizes are correct
const_assert_eq!(mem::size_of::<AmVideoContext>(), 0x400);
const_assert_eq!(mem::size_of::<AmVideoSetting>(), 0x14);

type AmDllVideoOpen = unsafe extern "C" fn(ctx: *mut AmVideoContext) -> usize;
type AmDllVideoClose = unsafe extern "C" fn(ctx: *mut AmVideoContext) -> usize;
type AmDllVideoSetResolution =
    unsafe extern "C" fn(ctx: *mut AmVideoContext, setting: *const AmVideoSetting) -> usize;
type AmDllVideoGetVBiosVersion =
    unsafe extern "C" fn(ctx: *mut AmVideoContext, dst: *mut u8, size: u32) -> usize;

struct AmVideo {
    lib: LibraryHandle,
    video_open: AmDllVideoOpen,
    video_close: AmDllVideoClose,
    video_set_resolution: AmDllVideoSetResolution,
    video_get_v_bios_version: AmDllVideoGetVBiosVersion,
    ctx: AmVideoContext,
}

#[derive(Debug)]
struct AmVideoError(usize);

impl AmVideo {
    fn new<T: AsRef<OsStr>>(name: T) -> Result<Self> {
        let name = name.as_ref();
        let lib = unsafe {
            let name: Vec<u16> = name.encode_wide().collect();
            LoadLibraryW(name.as_ptr())
        };
        if lib.is_null() {
            let e = Error::last_os_error();
            let name = name.to_string_lossy();
            let name = name.trim_end_matches('\0');
            return Err(e).with_context(|| format!("Failed to load '{}'", name));
        }
        let lib = LibraryHandle::new(lib);

        println!("Opened amVideo.dll @ {:?}", lib);

        // get functions
        let video_open: AmDllVideoOpen;
        let video_close: AmDllVideoClose;
        let video_set_resolution: AmDllVideoSetResolution;
        let video_get_v_bios_version: AmDllVideoGetVBiosVersion;
        unsafe {
            let am_dll_video_open = lib.get_func_named_ordinal("amDllVideoOpen", 1);
            let am_dll_video_close = lib.get_func_named_ordinal("amDllVideoClose", 2);
            let am_dll_video_set_resolution =
                lib.get_func_named_ordinal("amDllVideoSetResolution", 3);
            let am_dll_video_get_vbios_version =
                lib.get_func_named_ordinal("amDllVideoGetVBiosVersion", 4);

            let results = vec![
                &am_dll_video_open,
                &am_dll_video_close,
                &am_dll_video_set_resolution,
                &am_dll_video_get_vbios_version,
            ];
            let bad_funcs: Vec<_> = results
                .into_iter()
                .flat_map(|result| match result {
                    Ok(_) => None,
                    Err(e) => Some(e),
                })
                .map(|e| e.name())
                .collect();

            if !bad_funcs.is_empty() {
                return Err(anyhow!(
                    "Failed to find functions: {}",
                    bad_funcs.join(", ")
                ));
            }

            video_open = mem::transmute(am_dll_video_open?);
            video_close = mem::transmute(am_dll_video_close?);
            video_set_resolution = mem::transmute(am_dll_video_set_resolution?);
            video_get_v_bios_version = mem::transmute(am_dll_video_get_vbios_version?);

            println!("Loaded amDllVideoOpen @ {:?}", video_open);
            println!("Loaded amDllVideoClose @ {:?}", video_close);
            println!(
                "Loaded amDllVideoSetResolution @ {:?}",
                video_set_resolution
            );
            println!(
                "Loaded amDllVideoGetVBiosVersion @ {:?}",
                video_get_v_bios_version
            );
        }

        let ctx = AmVideoContext {
            version: 1,
            data: [0; AM_VIDEO_CONTEXT_DATA_SIZE],
        };

        Ok(Self {
            lib,
            video_open,
            video_close,
            video_set_resolution,
            video_get_v_bios_version,
            ctx,
        })
    }

    /// Enable amVideo's built-in error logging
    ///
    /// Offsets are for "amVideoNvidia Build:Jan 30 2015 18:51:29 ($Rev: 4624 $)"
    #[allow(dead_code)]
    fn enable_logging(&mut self) {
        unsafe {
            // Use `#[repr(transparent)]` here
            let amvideo_ptr = *self.lib as *mut u8;

            // Compute memory locations
            let validate_log_level = amvideo_ptr.add(0x505D4) as *mut u32;
            let log_level = amvideo_ptr.add(0x505D8) as *mut u32;

            *validate_log_level = 1;
            *log_level = 1;
        };
    }

    fn open(&mut self) -> Result<(), AmVideoError> {
        let result = unsafe { (self.video_open)(&mut self.ctx) };
        if result == 0 {
            Ok(())
        } else {
            Err(AmVideoError(result))
        }
    }

    fn set_resolution(&mut self, setting: &AmVideoSetting) -> Result<(), AmVideoError> {
        let result = unsafe { (self.video_set_resolution)(&mut self.ctx, setting) };
        if result == 0 {
            Ok(())
        } else {
            Err(AmVideoError(result))
        }
    }

    fn get_vbios_version(&mut self) -> Result<String> {
        let mut data = [0; 255];
        let result = unsafe {
            (self.video_get_v_bios_version)(&mut self.ctx, data.as_mut_ptr(), data.len() as u32)
        };
        if result != 0 {
            return Err(AmVideoError(result).into());
        }

        let data = data.split(|&c| c == 0).nth(0).unwrap_or(&data);
        let version =
            str::from_utf8(data).context("Failed to interpret VBIOS version string as UTF-8")?;
        Ok(version.to_string())
    }
}

impl Drop for AmVideo {
    fn drop(&mut self) {
        let result = unsafe { (self.video_close)(&mut self.ctx) };
        if result != 0 {
            eprintln!("Failed to close amVideo: {}", result);
        }
    }
}

impl fmt::Display for AmVideoError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "amVideo function failed: {}", self.0)
    }
}

impl StdError for AmVideoError {}

fn main() -> Result<()> {
    let name: OsString = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey("System\\Sega\\SystemProperty\\amVideo")
        .context("Failed to open 'System\\Sega\\SystemProperty\\amVideo'")?
        .get_value("name")
        .context("Failed to get amVideo 'name'")?;
    let mut amvideo = AmVideo::new(name)?;
    //amvideo.enable_logging();
    amvideo.open()?;

    // Get VBIOS version
    match amvideo
        .get_vbios_version()
        .context("Failed to get VBIOS version")
    {
        Ok(vbios_version) => println!("VBIOS Version: {}", vbios_version),
        Err(e) => eprintln!("{:?}", e),
    };

    // Set resolution
    let resolution = AmVideoSetting {
        version: 1,
        use_segatiming: 1,
        mode: AmVideoMode::Single,
        resolution_1: AmVideoResolution {
            width: 1920,
            height: 1080,
        },
        resolution_2: AmVideoResolution {
            width: 1920,
            height: 1080,
        },
    };
    println!("Attempting to set resolution: {:#?}", resolution);
    amvideo.set_resolution(&resolution)?;

    println!("Done");

    Ok(())
}
