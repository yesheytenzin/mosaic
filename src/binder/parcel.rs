// SPDX-License-Identifier: GPL-3.0-or-later

//! Writing a parcel, for the services the broker answers itself.
//!
//! A reply is the AIDL status word and then the values, in the order the generated
//! proxy reads them, and a value's encoding is libbinder's: integers native-endian,
//! booleans an int32, strings a count then UTF-16 and a terminator, a parcelable a
//! present-flag then its fields, and a binder object 24 bytes of
//! `flat_binder_object` followed by a 4-byte stability word.
//!
//! Every one of those is a place where writing too little is read as a *different
//! value* rather than as an error -- `Parcel::readInt32` on a short parcel yields
//! zero, `readStrongBinder` yields null -- so the shapes here are the contract.

/// A binder object's type word, from `Parcel::unflattenBinder`.
pub const BINDER_TYPE_BINDER: u32 = 0x73622a85;
/// A binder object that names a handle in the reader's own table.
pub const BINDER_TYPE_HANDLE: u32 = 0x73682a85;

/// `Parcel::kNonNullParcelableFlag`: a parcelable that is present.
const PARCELABLE_PRESENT: i32 = 1;

/// `Stability::Level` (`Stability.h`), written after every binder object.
/// A file descriptor inside a Parcel: an object word whose value is the number of
/// a descriptor in the receiving process's table. `Parcel::writeDupFileDescriptor`
/// writes one, and `readFileDescriptor` reads it back.
/// `B_PACK_CHARS('f', 'd', '*', B_TYPE_LARGE)`, which is 'f', 'd', '*' and the type
/// byte. This was `0x73662a85` -- 's', 'f', '*' -- which is the handle pattern with
/// the middle letter changed, and the framework rejects it as an invalid object
/// type, so a descriptor written with it is dropped rather than taken.
pub const BINDER_TYPE_FD: u32 = 0x66642a85;
pub const STABILITY_UNDECLARED: i32 = 0;
pub const STABILITY_SYSTEM: i32 = 12; /* 0b001100 */
pub const STABILITY_VINTF: i32 = 63; /* 0b111111 */

/// The transactions every binder object answers, from `IBinder.h` where the codes
/// are `B_PACK_CHARS`. Neither carries an interface token, and a proxy asks the
/// second before it will use the object it holds.
pub const PING_TRANSACTION: u32 = 0x5f504e47; /* ('_','P','N','G') */
pub const INTERFACE_TRANSACTION: u32 = 0x5f4e5446; /* ('_','N','T','F') */

#[derive(Default)]
pub struct Parcel {
    bytes: Vec<u8>,
}

impl Parcel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn i32(&mut self, value: i32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub fn i64(&mut self, value: i64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub fn boolean(&mut self, value: bool) {
        self.i32(i32::from(value));
    }

    /// The exception code that means "no exception". Everything else follows it.
    pub fn ok(&mut self) {
        self.i32(0);
    }

    /// An array of nothing: a count of zero.
    pub fn empty_array(&mut self) {
        self.i32(0);
    }

    /// A parcelable that is present: the AIDL flag, then its fields.
    pub fn parcelable_present(&mut self) {
        self.i32(PARCELABLE_PRESENT);
    }

    /// A `String16`: the unit count, the units, a terminator, and padding to four
    /// bytes. ASCII in, because the strings the broker answers with are.
    pub fn string16(&mut self, text: &str) {
        let units: Vec<u16> = text.encode_utf16().collect();
        self.i32(units.len() as i32);
        for unit in &units {
            self.bytes.extend_from_slice(&unit.to_le_bytes());
        }
        self.bytes.extend_from_slice(&[0, 0]);
        while self.bytes.len() % 4 != 0 {
            self.bytes.push(0);
        }
    }

    /// A binder object that is not there: `BINDER_TYPE_BINDER` with both pointers
    /// zero, then the stability word a null must carry. Writing *nothing* instead
    /// is read as `BAD_TYPE`, and writing a type of zero as the same.
    pub fn null_binder(&mut self) {
        self.binder_object(BINDER_TYPE_BINDER, 0, 0, STABILITY_UNDECLARED);
    }

    /// A place for a descriptor word. The number is written by the side that
    /// holds the descriptor -- the one that can open it -- so what goes here is
    /// the shape and nothing else. Returns where the word begins.
    pub fn fd_placeholder(&mut self) -> u32 {
        let offset = self.bytes.len() as u32;
        // A descriptor object is 24 bytes, not 28: the stability word a binder
        // object carries is written for handles and binders, and a file descriptor
        // is neither. Reserving 28 put the *second* descriptor four bytes past
        // where the reader looks for it, and what the reader said was
        // "offset 32 that is not in the object list".
        for _ in 0..24 {
            self.bytes.push(0);
        }
        offset
    }

    /// A handle to an object in another process, as the shim hands out for the
    /// objects the broker hosts. Returns where the object word begins, which is
    /// what the side sending the answer needs to name the object it wants to hand
    /// back.
    pub fn handle_binder(&mut self, handle: u32) -> u32 {
        let offset = self.bytes.len() as u32;
        self.binder_object(BINDER_TYPE_HANDLE, u64::from(handle), 0, STABILITY_SYSTEM);
        offset
    }

    fn binder_object(&mut self, kind: u32, value: u64, cookie: u64, stability: i32) {
        self.i32(kind as i32);
        self.i32(0); /* flags */
        self.i64(value as i64);
        self.i64(cookie as i64);
        self.i32(stability);
    }

    /// Raw bytes, for a value built elsewhere and copied in whole.
    pub fn raw(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// A Java AIDL *call*'s parcel: the header a Java `Parcel` begins with, the
/// interface descriptor, then the arguments.
///
/// This shape is not optional in either direction. A Java receiver's `onTransact`
/// starts with `enforceInterface`, and a parcel that does not begin with the header
/// is refused before a single argument is read -- `Expecting header 0x53595354 but
/// found 0x0. Mixing copies of libbinder?` and then `Binder invocation to an
/// incorrect interface`.
///
/// The header is twelve bytes -- two words the Java `Parcel` writes for itself and
/// the `TSYS` marker -- and then the descriptor as a count of UTF-16 units
/// *including* its terminator, the units themselves, the terminator, and padding to
/// four. Services have been *reading* this shape since the request layout was
/// worked out; a service that has to *call back* into Java has to write it, and the
/// health HAL's callback is the first that does.
pub fn java_call(descriptor: &str, args: &[u8]) -> Vec<u8> {
    let mut data = vec![0x00, 0x00, 0x00, 0x80, 0xff, 0xff, 0xff, 0xff];
    data.extend_from_slice(b"TSYS");
    // The count is the units *without* the terminator, which is what the platform's
    // own writer puts on the wire: a forty-character descriptor goes out with forty.
    let units = descriptor.encode_utf16().count();
    data.extend_from_slice(&(units as i32).to_le_bytes());
    for unit in descriptor.encode_utf16() {
        data.extend_from_slice(&unit.to_le_bytes());
    }
    data.extend_from_slice(&[0, 0]);
    while data.len() % 4 != 0 {
        data.push(0);
    }
    data.extend_from_slice(args);
    data
}

/// Where the arguments start, found rather than counted.
///
/// A request is an AIDL header, the interface token as a string16, and then the
/// arguments. The token is located by its own text: its length prefix sits in
/// front of the text and the padding after it follows from that length, but
/// counting from the front has been wrong before (the header in front of the token
/// is not a fixed size across interfaces), and a search cannot be wrong about
/// where the text is.
/// Reading the arguments of a request.
///
/// The mirror of [`Parcel`], and deliberately just as small: the fields an AIDL
/// method takes, in declaration order. A short buffer reads as zero rather than
/// panicking, because a peer that sends less than it promised must not be able to
/// take this side down.
pub struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    fn take(&mut self, count: usize) -> &'a [u8] {
        if self.at + count > self.data.len() {
            return &[];
        }
        let slice = &self.data[self.at..self.at + count];
        self.at += count;
        slice
    }

    pub fn i32(&mut self) -> i32 {
        let bytes = self.take(4);
        if bytes.len() < 4 {
            return 0;
        }
        i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }

    /// A float, which is its four bytes rather than a converted integer: an `i32`
    /// read of the same bytes is the bit pattern, and comparing that to a float is
    /// how a test says nothing at all.
    pub fn f32(&mut self) -> f32 {
        let bytes = self.take(4);
        if bytes.len() < 4 {
            return 0.0;
        }
        f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }

    /// One byte, which is what a bool is in a flattened stream (a *Parcel* writes
    /// four, and the two are different conventions).
    pub fn i8(&mut self) -> i8 {
        let bytes = self.take(1);
        if bytes.is_empty() {
            return 0;
        }
        bytes[0] as i8
    }

    pub fn i64(&mut self) -> i64 {
        let bytes = self.take(8);
        if bytes.len() < 8 {
            return 0;
        }
        let mut word = [0u8; 8];
        word.copy_from_slice(bytes);
        i64::from_le_bytes(word)
    }

    /// A string16. `None` is the wire's null string, a count of -1.
    pub fn string(&mut self) -> Option<String> {
        let count = self.i32();
        if count < 0 {
            return None;
        }
        let units = self.take(count as usize * 2);
        let mut text = String::new();
        for pair in units.chunks_exact(2) {
            let unit = u16::from_le_bytes([pair[0], pair[1]]);
            if unit != 0 {
                text.push(char::from_u32(unit as u32).unwrap_or('?'));
            }
        }
        // Past the terminator, then up to the next four byte boundary.
        self.at = (self.at + 2 + 3) & !3;
        Some(text)
    }

    /// An array of int64: a count and then the values.
    pub fn i64s(&mut self) -> Vec<i64> {
        let count = self.i32();
        if count <= 0 {
            return Vec::new();
        }
        (0..count).map(|_| self.i64()).collect()
    }

    /// An array of ints: a count and then the ints.
    pub fn i32s(&mut self) -> Vec<i32> {
        let count = self.i32();
        if count <= 0 {
            return Vec::new();
        }
        (0..count).map(|_| self.i32()).collect()
    }

    /// A `String8`, which is not the same shape as a Java string: a *byte* length, then the bytes
    /// and their terminator, padded out to four. `Parcel::writeString8` and `readString8` are the
    /// pair, and a service that answers one to a caller that reads the other is off by the padding.
    pub fn string8(&mut self) -> Option<String> {
        let len = self.i32();
        if len < 0 {
            return None;
        }
        let bytes = self.take(len as usize);
        self.take(1); // the terminator
        while self.at % 4 != 0 {
            self.at += 1;
        }
        Some(String::from_utf8_lossy(bytes).into_owned())
    }

    /// An array of strings: a count and then the strings.
    pub fn strings(&mut self) -> Vec<String> {
        let count = self.i32();
        if count <= 0 {
            return Vec::new();
        }
        (0..count).filter_map(|_| self.string()).collect()
    }

    /// Whether the next word is the presence flag of a parcelable argument, and
    /// consumes it if so. AIDL writes `1` in front of a parcelable's fields; this
    /// accepts a request that does not carry it as well, because the flag and a
    /// string's length word share a position and only one of them can be `1` in
    /// anything a caller would send.
    pub fn parcelable_present(&mut self) {
        if self.at + 4 <= self.data.len() && self.i32_at(self.at) == 1 {
            self.at += 4;
        }
    }

    fn i32_at(&self, at: usize) -> i32 {
        if at + 4 > self.data.len() {
            return 0;
        }
        i32::from_le_bytes([
            self.data[at],
            self.data[at + 1],
            self.data[at + 2],
            self.data[at + 3],
        ])
    }

    /// A UTF-8 view of what is left, for a caller that wants the rest.
    pub fn rest(&self) -> &'a [u8] {
        &self.data[self.at.min(self.data.len())..]
    }
}

/// Whether a request carries this interface's token.
///
/// The token is how a request says which interface it is for, and it is the only
/// thing that can: two interfaces can share a name and their transaction codes
/// overlap, so a server that answers by code alone answers the wrong method for
/// one of them.
pub fn carries_token(data: &[u8], descriptor: &str) -> bool {
    let mut needle = Vec::with_capacity(descriptor.len() * 2);
    for unit in descriptor.encode_utf16() {
        needle.extend_from_slice(&unit.to_le_bytes());
    }
    !needle.is_empty()
        && data
            .windows(needle.len())
            .any(|window| window == needle.as_slice())
}

pub fn args_after<'a>(data: &'a [u8], descriptor: &str) -> &'a [u8] {
    let mut needle = Vec::with_capacity(descriptor.len() * 2);
    for unit in descriptor.encode_utf16() {
        needle.extend_from_slice(&unit.to_le_bytes());
    }
    let start = match data
        .windows(needle.len())
        .position(|window| window == needle.as_slice())
    {
        Some(at) => at,
        None => return &[],
    };
    // The string16's count is the int32 just before the text; the arguments begin
    // after the text, its terminator and the padding to four bytes.
    //
    // Whether the count includes the terminator differs between the two doors this
    // reads requests from (the platform's AIDL writer counts the units without it,
    // the one the framework reaches through `libbinder_ndk` counts it in), so the
    // bytes decide: if the last unit the count covers is a NUL, the terminator is
    // already inside it.
    let text_len = if start >= 4 {
        let count = i32::from_le_bytes([
            data[start - 4],
            data[start - 3],
            data[start - 2],
            data[start - 1],
        ]);
        if count > 0 && (count as usize) * 2 <= data.len() {
            (count as usize) * 2
        } else {
            needle.len()
        }
    } else {
        needle.len()
    };
    let text_end = start + text_len;
    let terminated = text_len >= 2 && data[text_end - 2] == 0 && data[text_end - 1] == 0;
    let end = if terminated {
        (text_end + 3) & !3
    } else {
        (text_end + 2 + 3) & !3
    };
    if end >= data.len() {
        &[]
    } else {
        &data[end..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_string16_is_a_count_units_a_terminator_and_padding() {
        // Four units: 4 + 8 + 2 -> padded to 16.
        let mut parcel = Parcel::new();
        parcel.string16("abcd");
        let bytes = parcel.into_bytes();
        assert_eq!(bytes.len() % 4, 0);
        assert_eq!(&bytes[..4], &4i32.to_le_bytes());
        assert_eq!(&bytes[4..6], &[b'a', 0]);
        assert_eq!(&bytes[bytes.len() - 4..], &[0, 0, 0, 0]);
    }

    #[test]
    fn a_null_binder_is_a_type_and_two_zero_pointers_and_a_stability_word() {
        let mut parcel = Parcel::new();
        parcel.null_binder();
        let bytes = parcel.into_bytes();
        assert_eq!(bytes.len(), 28);
        assert_eq!(&bytes[..4], &BINDER_TYPE_BINDER.to_le_bytes());
        assert!(bytes[4..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn a_handle_carries_its_number_and_a_declared_stability() {
        let mut parcel = Parcel::new();
        parcel.handle_binder(17);
        let bytes = parcel.into_bytes();
        assert_eq!(&bytes[..4], &BINDER_TYPE_HANDLE.to_le_bytes());
        assert_eq!(&bytes[8..12], &17u32.to_le_bytes());
        assert_eq!(&bytes[24..28], &STABILITY_SYSTEM.to_le_bytes());
    }
}
