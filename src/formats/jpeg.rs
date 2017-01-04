//! Metadata of JPEG images.

use std::io::BufRead;
use std::fmt;

use byteorder::{ReadBytesExt, BigEndian};

use types::{Result, Dimensions, Error};
use traits::LoadableMetadata;
use utils::BufReadExt;

/// Coding process used in an image.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum CodingProcess {
    /// Sequential DCT (discrete cosine transform).
    DctSequential,
    /// Progressive DCT.
    DctProgressive,
    /// Lossless coding.
    Lossless
}


impl fmt::Display for CodingProcess {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match *self {
            CodingProcess::DctSequential => "Sequential DCT",
            CodingProcess::DctProgressive => "Progressive DCT",
            CodingProcess::Lossless => "Lossless",
        })
    }
}

impl CodingProcess {
    fn from_marker(marker: u8) -> Option<CodingProcess> {
        match marker {
            0xc0 | 0xc1 | 0xc5 | 0xc9 | 0xcd => Some(CodingProcess::DctSequential),
            0xc2 | 0xc6 | 0xca | 0xce => Some(CodingProcess::DctProgressive),
            0xc3 | 0xc7 | 0xcb | 0xcf => Some(CodingProcess::Lossless),
            _ => None
        }
    }
}

/// Entropy coding method used in an image.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum EntropyCoding {
    /// Huffman coding.
    Huffman,
    /// Arithmetic coding.
    Arithmetic
}

impl fmt::Display for EntropyCoding {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match *self {
            EntropyCoding::Huffman => "Huffman",
            EntropyCoding::Arithmetic => "Arithmetic",
        })
    }
}

impl EntropyCoding {
    fn from_marker(marker: u8) -> Option<EntropyCoding> {
        match marker {
            0xc0 | 0xc1 | 0xc2 | 0xc3 | 0xc5 | 0xc6 | 0xc7 => Some(EntropyCoding::Huffman),
            0xc9 | 0xca | 0xcb | 0xcd | 0xce | 0xcf => Some(EntropyCoding::Arithmetic),
            _ => None
        }
    }
}

/// Represents metadata of a JPEG image.
///
/// It provides information contained in JPEG frame header, including image dimensions,
/// coding process type and entropy coding type.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Metadata {
    /// Image size.
    pub dimensions: Dimensions,
    /// Sample precision (in bits).
    pub sample_precision: u8,
    /// Image coding process type.
    pub coding_process: CodingProcess,
    /// Image entropy coding type.
    pub entropy_coding: EntropyCoding,
    /// Whether this image uses a baseline DCT encoding.
    pub baseline: bool,
    /// Whether this image uses a differential encoding.
    pub differential: bool,
}

fn find_marker<R: ?Sized, F>(r: &mut R, name: &str, mut matcher: F) -> Result<u8>
    where R: BufRead, F: FnMut(u8) -> bool
{
    loop {
        if try!(r.skip_until(0xff)) == 0 {
            return Err(unexpected_eof!("when searching for {} marker", name));
        }
        let marker_type = try_if_eof!(r.read_u8(), "when reading marker type");
        if marker_type == 0 { continue; }  // skip "stuffed" byte

        if matcher(marker_type) {
            return Ok(marker_type);
        }
    }
}


impl LoadableMetadata for Metadata {
    fn load<R: ?Sized + BufRead>(r: &mut R) -> Result<Metadata> {
        // read SOI marker, it must be present in all JPEG files
        try!(find_marker(r, "SOI", |m| m == 0xd8));

        // Read the APP marker to determine wether or not the file is stored
        // as a JFIF file or as a EXIF file. Some cameras store files as EXIF
        // while others store it as JFIF.
        let app_marker = try!(find_marker(r, "APP", is_app_marker));

        let length = try_if_eof!(r.read_u16::<BigEndian>(), "when reading APP marker size");
        if length <= 8 {
            return Err(invalid_format!("invalid JPG APP header length: {}", length))
        }

        // Checking if the file is EXIF or JFIF.
        // XXX: I believe checking if the file has a APP0 or APP1 marker should
        // XXX: be enough but that could be wrong if there are more exotic JPEGS
        // XXX: out there. If you encounter a file that is parsed wrong, try to
        // XXX: determine the JPEG container that is used
        match try!(container_type_from_app_marker(app_marker)) {
            ContainerType::JFIF => load_jfif(r),
            //ContainerType::EXIF => Err(invalid_format!("EXIF file format is not supported"))
            ContainerType::EXIF => load_exif(r)
        }

    }

}

fn load_jfif<R: ?Sized + BufRead>(r: &mut R) -> Result<Metadata> {
    // read SOF marker, it must also be present in all JPEG files
    let marker = try!(find_marker(r, "SOF", is_sof_marker));

    // read and check SOF marker length
    let size = try_if_eof!(r.read_u16::<BigEndian>(), "when reading SOF marker payload size");
    if size <= 8 {  // 2 bytes for the length itself, 6 bytes is the minimum header size
        return Err(invalid_format!("invalid JPEG frame header size: {}", size));
    }

    //The marker for dimension can either be SOF0 or SOF2 depending on if the
    //image is a baseline or progressive DCT-based jpeg. If the marker
    //is c0 then it is baseline, c2 is progressive

    // read sample precision
    let sample_precision = try_if_eof!(r.read_u8(), "when reading sample precision of the frame");

    // read height and width
    let h = try_if_eof!(r.read_u16::<BigEndian>(), "when reading JPEG frame height");
    let w = try_if_eof!(r.read_u16::<BigEndian>(), "when reading JPEG frame width");
    // TODO: handle h == 0 (we need to read a DNL marker after the first scan)

    // there is only one baseline DCT marker, naturally
    let baseline = marker == 0xc0;

    let differential = match marker {
        0xc0 | 0xc1 | 0xc2 | 0xc3 | 0xc9 | 0xca | 0xcb => false,
        0xc5 | 0xc6 | 0xc7 | 0xcd | 0xce | 0xcf => true,
        _ => unreachable!(),  // because we are inside a valid SOF marker
    };

    // unwrap can't fail, we're inside a valid SOF marker
    let coding_process = CodingProcess::from_marker(marker).unwrap();
    let entropy_coding = EntropyCoding::from_marker(marker).unwrap();

    Ok(Metadata {
        dimensions: (w, h).into(),
        sample_precision: sample_precision,
        coding_process: coding_process,
        entropy_coding: entropy_coding,
        baseline: baseline,
        differential: differential,
    })
}
fn load_exif<R: ?Sized + BufRead>(r: &mut R) -> Result<Metadata> {
    // The first 6 bytes should be the string "EXIF" followed by two null bytes.
    // If this is not the case, we don't have an EXIF file
    const EXIF_IDENTIFIER_LENGTH: usize= 6;
    let mut buffer = [0; EXIF_IDENTIFIER_LENGTH];
    try!(r.read_exact(&mut buffer));
    if buffer != [0x45, 0x78, 0x69, 0x66, 0x00, 0x00]
    {
        return Err(invalid_format!("JPEG file with APP1 marker is not EXIF"));
    }

    //Reading the byte allign of the exif content. Some EIXF files use big
    //endian while some use little endian files
    unimplemented!()
}

fn load_exif_with_endianness<R: ?Sized + BufRead, E: >

fn is_sof_marker(value: u8) -> bool {
    match value {
        // no 0xC4, 0xC8 and 0xCC, they are not SOF markers
        0xc0 | 0xc1 | 0xc2 | 0xc3 | 0xc5 | 0xc6 | 0xc7 | 0xc9 |
        0xca | 0xcb | 0xcd | 0xce | 0xcf => true,
        _ => false
    }
}

fn is_app_marker(value: u8) -> bool
{
    match value {
        0xe0 | 0xe1 => true,
        _ => false
    }
}


/**
  Determines wether or not this is an EXIF file based on the APPn marker
*/
fn container_type_from_app_marker(marker: u8) -> Result<ContainerType>
{
    match marker {
        0xe0 => Ok(ContainerType::JFIF),
        0xe1 => Ok(ContainerType::EXIF),
        _ => Err(invalid_format!("JPEG file is neighter JFIF nor EXIF"))
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum ContainerType {
    JFIF,
    EXIF
}

