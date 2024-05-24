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

use std::error::Error;
use std::fmt;
use std::io;
use std::ops::Deref;

use winapi::shared::minwindef::{FARPROC, HMODULE};
use winapi::um::libloaderapi::{FreeLibrary, GetProcAddress};

/// RAII guard around a dynamically loaded module
#[repr(transparent)]
pub struct LibraryHandle {
    handle: HMODULE,
}

#[derive(Debug)]
pub struct FunctionGetError<'a> {
    name: &'a str,
    source: io::Error,
}

impl LibraryHandle {
    pub const fn new(handle: HMODULE) -> Self {
        Self { handle }
    }

    pub unsafe fn get_func_named_ordinal<'a>(
        &self,
        name: &'a str,
        ordinal: u16,
    ) -> Result<FARPROC, FunctionGetError<'a>> {
        let func = GetProcAddress(self.handle, ordinal as *const _);

        if !func.is_null() {
            Ok(func)
        } else {
            Err(FunctionGetError {
                name,
                source: io::Error::last_os_error(),
            })
        }
    }
}

impl fmt::Debug for LibraryHandle {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&self.handle, f)
    }
}

impl Deref for LibraryHandle {
    type Target = HMODULE;

    fn deref(&self) -> &Self::Target {
        &self.handle
    }
}

impl Drop for LibraryHandle {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { FreeLibrary(self.handle) };
        }
    }
}

impl FunctionGetError<'_> {
    pub const fn name(&self) -> &str {
        self.name
    }
}

impl fmt::Display for FunctionGetError<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Failed to get function '{}'", self.name)
    }
}

impl Error for FunctionGetError<'_> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}
