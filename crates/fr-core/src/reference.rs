//! Portable sketch reference images. Pixels are embedded, never an external path.
//! Decoding is bounded and cached across the inexpensive clones used by undo.

use std::fs::File;
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::{Arc, OnceLock};

use base64::{Engine, engine::general_purpose::STANDARD};
use glam::DVec2;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use serde::{Deserialize, Serialize};

pub const MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_PNG_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_PIXELS: u64 = 4 * 1024 * 1024;
pub const MAX_SIDE: u32 = 8192;
const MAX_MM: f64 = 1e9;

#[derive(Debug)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    /// Top to bottom, unpremultiplied RGBA8.
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReferenceImage {
    /// Normalized PNG without source metadata. Kept private so the cache cannot go stale.
    png: Arc<str>,
    pub name: String,
    pub pixel_width: u32,
    pub pixel_height: u32,
    /// Lower-left corner, in sketch millimetres.
    pub origin: DVec2,
    pub width: f64,
    /// Counterclockwise, in degrees in the sketch plane.
    pub rotation: f64,
    pub opacity: f32,
    pub visible: bool,
    #[serde(skip)]
    decoded: Arc<OnceLock<Result<Arc<Pixels>, String>>>,
}

impl PartialEq for ReferenceImage {
    fn eq(&self, other: &Self) -> bool {
        (Arc::ptr_eq(&self.png, &other.png) || self.png == other.png)
            && self.name == other.name && self.pixel_width == other.pixel_width
            && self.pixel_height == other.pixel_height && self.origin == other.origin
            && self.width == other.width && self.rotation == other.rotation
            && self.opacity == other.opacity && self.visible == other.visible
    }
}

fn decode(bytes: &[u8], png_only: bool) -> Result<Pixels, String> {
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|e| format!("Cannot read the reference image: {e}"))?;
    match reader.format() {
        Some(ImageFormat::Png) => {}
        Some(ImageFormat::Jpeg) if !png_only => {}
        _ => return Err("Reference images must be PNG or JPEG.".into()),
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|e| format!("Cannot decode the reference image: {e}"))?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err("Reference images must contain at most 4 megapixels.".into());
    }
    let orientation = decoder.orientation().map_err(|e| format!("Cannot read the reference image orientation: {e}"))?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(|e| format!("Cannot decode the reference image: {e}"))?;
    image.apply_orientation(orientation);
    let image = image.into_rgba8();
    Ok(Pixels { width: image.width(), height: image.height(), rgba: image.into_raw() })
}

impl ReferenceImage {
    /// Reads at most the import limit, including when a file changes while being read.
    pub fn from_file(path: &Path, width: f64) -> Result<Self, String> {
        let metadata = std::fs::metadata(path).map_err(|e| format!("Cannot inspect the reference image: {e}"))?;
        if !metadata.is_file() { return Err("Choose a regular PNG or JPEG file.".into()); }
        if metadata.len() > MAX_INPUT_BYTES as u64 {
            return Err("Reference images must be 8 MiB or smaller. Resize the image before importing it.".into());
        }
        let mut bytes = Vec::new();
        File::open(path).map_err(|e| format!("Cannot open the reference image: {e}"))?
            .take(MAX_INPUT_BYTES as u64 + 1).read_to_end(&mut bytes)
            .map_err(|e| format!("Cannot read the reference image: {e}"))?;
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("Reference image");
        Self::from_bytes(name, &bytes, width)
    }

    pub fn from_bytes(name: &str, bytes: &[u8], width: f64) -> Result<Self, String> {
        if bytes.len() > MAX_INPUT_BYTES {
            return Err("Reference images must be 8 MiB or smaller. Resize the image before importing it.".into());
        }
        let pixels = Arc::new(decode(bytes, false)?);
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, pixels.width, pixels.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().map_err(|e| format!("Cannot embed the reference image: {e}"))?;
            writer.write_image_data(&pixels.rgba).map_err(|e| format!("Cannot embed the reference image: {e}"))?;
        }
        if png.len() > MAX_PNG_BYTES {
            return Err("The decoded image is too large to embed. Resize it before importing.".into());
        }
        let basename = name.rsplit(['/', '\\']).next().unwrap_or("Reference image");
        let name: String = basename.chars().filter(|c| !c.is_control()).take(128).collect();
        let image = Self {
            png: STANDARD.encode(png).into(),
            name: if name.is_empty() { "Reference image".into() } else { name },
            pixel_width: pixels.width, pixel_height: pixels.height,
            origin: DVec2::ZERO, width, rotation: 0.0, opacity: 0.5, visible: true,
            decoded: Arc::new(OnceLock::from(Ok(pixels))),
        };
        image.validate()?;
        Ok(image)
    }

    /// Checks both placement and the actual embedded pixels; repeated calls share a decode cache.
    pub fn validate(&self) -> Result<(), String> {
        self.validate_placement()?;
        let pixels = self.pixels()?;
        if pixels.width != self.pixel_width || pixels.height != self.pixel_height {
            return Err("Reference image dimensions do not match its embedded pixels.".into());
        }
        Ok(())
    }

    fn validate_placement(&self) -> Result<(), String> {
        if self.name.chars().count() > 128 || self.name.contains(['/', '\\']) || self.name.chars().any(char::is_control) {
            return Err("The reference image name must be a short filename, without a path.".into());
        }
        if self.pixel_width == 0 || self.pixel_height == 0 || self.pixel_width > MAX_SIDE || self.pixel_height > MAX_SIDE
            || u64::from(self.pixel_width) * u64::from(self.pixel_height) > MAX_PIXELS {
            return Err("Reference image dimensions exceed the supported limits.".into());
        }
        if !self.origin.is_finite() || self.origin.abs().max_element() > MAX_MM
            || !self.width.is_finite() || self.width <= 1e-9 || self.width > MAX_MM
            || !self.rotation.is_finite() || self.rotation.abs() > 1e6
            || !self.opacity.is_finite() || !(0.0..=1.0).contains(&self.opacity) {
            return Err("Reference image placement needs finite coordinates, a positive width, and opacity from 0 to 1.".into());
        }
        if self.corners().iter().any(|p| !p.is_finite() || p.abs().max_element() > MAX_MM) {
            return Err("Reference image corners are outside the supported coordinate range.".into());
        }
        Ok(())
    }

    pub fn pixels(&self) -> Result<Arc<Pixels>, String> {
        self.decoded.get_or_init(|| {
            if self.png.len() > MAX_PNG_BYTES.div_ceil(3) * 4 {
                return Err("The embedded reference image is too large.".into());
            }
            let bytes = STANDARD.decode(self.png.as_bytes()).map_err(|_| "Invalid embedded reference image encoding.".to_owned())?;
            if bytes.len() > MAX_PNG_BYTES { return Err("The embedded reference image is too large.".into()); }
            decode(&bytes, true).map(Arc::new)
        }).clone()
    }

    /// Cheap identity for a GPU texture cache; changing placement does not replace the texture.
    pub fn same_pixels(&self, other: &Self) -> bool { Arc::ptr_eq(&self.png, &other.png) }

    /// The embedded pixels as a PNG file, for an exported script's sidecar.
    pub fn png_bytes(&self) -> Result<Vec<u8>, String> {
        STANDARD.decode(self.png.as_bytes()).map_err(|_| "Invalid embedded reference image encoding.".to_owned())
    }

    /// How many bytes the image takes inside a JSON document.
    pub fn embedded_len(&self) -> usize { self.png.len() }

    /// The same placement with another file's pixels.
    pub fn with_pixels_from(&self, path: &Path) -> Result<Self, String> {
        let mut image = Self::from_file(path, self.width)?;
        image.origin = self.origin;
        image.rotation = self.rotation;
        image.opacity = self.opacity;
        image.visible = self.visible;
        if !self.name.is_empty() { image.name = self.name.clone(); }
        Ok(image)
    }

    pub fn height(&self) -> f64 { self.width * f64::from(self.pixel_height) / f64::from(self.pixel_width) }

    /// Bottom-left, bottom-right, top-right, top-left in sketch coordinates.
    pub fn corners(&self) -> [DVec2; 4] {
        let (s, c) = self.rotation.to_radians().sin_cos();
        let x = DVec2::new(c, s) * self.width;
        let y = DVec2::new(-s, c) * self.height();
        [self.origin, self.origin + x, self.origin + x + y, self.origin + y]
    }

    /// Rescale so the selected distance has the known size. The first clicked point stays put.
    pub fn calibrate(&mut self, first: DVec2, second: DVec2, distance: f64) -> Result<(), String> {
        let measured = first.distance(second);
        if !first.is_finite() || !second.is_finite() || !distance.is_finite() || distance <= 1e-9 || measured <= 1e-9 {
            return Err("Choose two different image points and enter a positive known distance.".into());
        }
        let ratio = distance / measured;
        let mut candidate = self.clone();
        candidate.width *= ratio;
        candidate.origin = first + (candidate.origin - first) * ratio;
        candidate.validate_placement()?;
        *self = candidate;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut data = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut data, w, h);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header().unwrap().write_image_data(&vec![127; w as usize * h as usize * 4]).unwrap();
        }
        data
    }

    #[test]
    fn normalized_embedded_image_roundtrip_and_shared_undo_pixels() {
        let image = ReferenceImage::from_bytes("/private/folder/picture.png", &png(12, 6), 40.0).unwrap();
        assert_eq!(image.name, "picture.png");
        assert_eq!(image.height(), 20.0);
        let clone = image.clone();
        assert!(image.same_pixels(&clone));
        assert!(Arc::ptr_eq(&image.pixels().unwrap(), &clone.pixels().unwrap()));
        let json = serde_json::to_string(&image).unwrap();
        assert!(!json.contains("/private/"));
        assert!(!json.contains("decoded"));
        let restored: ReferenceImage = serde_json::from_str(&json).unwrap();
        restored.validate().unwrap();
        assert_eq!(restored, image);
        assert_eq!(restored.pixels().unwrap().rgba, image.pixels().unwrap().rgba);
    }

    #[test]
    fn jpeg_import_and_metadata_free_png_storage() {
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut bytes).encode(&[255, 0, 0, 0, 255, 0], 2, 1, image::ExtendedColorType::Rgb8).unwrap();
        // A phone-style EXIF orientation: rotate the stored landscape pixels 90 degrees.
        let exif = [b'E', b'x', b'i', b'f', 0, 0, b'I', b'I', 42, 0, 8, 0, 0, 0,
            1, 0, 0x12, 1, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0];
        let mut oriented = bytes[..2].to_vec();
        oriented.extend_from_slice(&[0xff, 0xe1, 0, 34]);
        oriented.extend_from_slice(&exif);
        oriented.extend_from_slice(&bytes[2..]);
        let image = ReferenceImage::from_bytes("photo.jpg", &bytes, 20.0).unwrap();
        assert!(STANDARD.decode(image.png.as_bytes()).unwrap().starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!((image.pixel_width, image.pixel_height), (2, 1));
        let oriented = ReferenceImage::from_bytes("phone.jpg", &oriented, 20.0).unwrap();
        assert_eq!((oriented.pixel_width, oriented.pixel_height), (1, 2));
        assert!(!STANDARD.decode(oriented.png.as_bytes()).unwrap().windows(4).any(|w| w == b"Exif"));
    }

    #[test]
    fn calibration_preserves_first_point_with_rotation_and_rejects_bad_scale_atomically() {
        let mut image = ReferenceImage::from_bytes("a.png", &png(2, 1), 40.0).unwrap();
        image.origin = DVec2::new(3.0, 4.0);
        image.rotation = 45.0;
        let anchor = image.corners()[2];
        let before = image.corners();
        image.calibrate(anchor, anchor + DVec2::new(3.0, 4.0), 10.0).unwrap();
        assert_eq!(image.width, 80.0);
        assert!(image.corners()[2].distance(anchor) < 1e-12);
        for (old, new) in before.into_iter().zip(image.corners()) { assert!((new - anchor).distance((old - anchor) * 2.0) < 1e-12); }
        let before = image.clone();
        assert!(image.calibrate(anchor, anchor, 10.0).is_err());
        assert!(image.calibrate(anchor, anchor + DVec2::X, f64::INFINITY).is_err());
        assert!(image.calibrate(anchor, anchor + DVec2::X, 1e30).is_err());
        assert_eq!(image, before);
    }

    #[test]
    fn rejects_invalid_oversized_and_mismatched_images() {
        assert!(ReferenceImage::from_bytes("a.png", b"not an image", 10.0).is_err());
        assert!(ReferenceImage::from_bytes("a.png", &vec![0; MAX_INPUT_BYTES + 1], 10.0).is_err());
        assert!(ReferenceImage::from_bytes("a.png", &png(MAX_SIDE + 1, 1), 10.0).is_err());
        assert!(ReferenceImage::from_bytes("a.png", &png(2049, 2049), 10.0).is_err());
        let mut image = ReferenceImage::from_bytes("a.png", &png(2, 1), 10.0).unwrap();
        image.pixel_width = 3;
        assert!(image.validate().is_err());
        image.pixel_width = 2;
        image.opacity = f32::NAN;
        assert!(image.validate().is_err());
        image.opacity = 0.5;
        let mut value = serde_json::to_value(image).unwrap();
        value["png"] = serde_json::json!("not base64");
        assert!(serde_json::from_value::<ReferenceImage>(value).unwrap().validate().is_err());
    }
}
