//! Images the person sends Moneypenny with a message. Each is checked (one of four formats, by
//! its bytes as well as its name; at most `[assistant.gateway] image_bytes`), kept once per
//! person by its SHA-256 in the pages directory, and named by that digest in her conversation
//! log and in the job a Station takes; the Station reads its bytes from the gateway.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use ai6::directorates::section_00::content::IMAGE_MEDIA_TYPES;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::pages::sha256_hex;

/// An image as the app's message frame carries it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AppImage {
    pub(crate) media_type: String,
    /// Standard base64.
    pub(crate) data: String,
}

/// An image as it is kept: what the log and the job name it by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct KeptImage {
    pub(crate) sha256: String,
    pub(crate) media_type: String,
    pub(crate) bytes: u64,
}

impl KeptImage {
    /// The images of a message as its log entry's `data` holds them.
    pub(crate) fn data(images: &[Self]) -> Value {
        json!({ "images": images })
    }
}

/// The format of `bytes` by their signature, when it is one of [`IMAGE_MEDIA_TYPES`].
pub(crate) fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else {
        None
    }
}

/// The bytes of `image` when it is a well-formed image of its declared format within `most`
/// bytes; otherwise why not, in words for the person.
pub(crate) fn check(image: &AppImage, most: ai6::Limit) -> Result<Vec<u8>, String> {
    if !IMAGE_MEDIA_TYPES.contains(&image.media_type.as_str()) {
        return Err(format!(
            "{} is not an image she takes; send PNG, JPEG, WebP or GIF",
            image.media_type
        ));
    }
    if !most.allows(image.data.len().saturating_mul(3) / 4) {
        return Err(format!("an image may hold {most} bytes"));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(image.data.as_bytes())
        .map_err(|_| "an image's data is not base64".to_owned())?;
    if !most.allows(bytes.len()) {
        return Err(format!("an image may hold {most} bytes"));
    }
    if sniff(&bytes) != Some(image.media_type.as_str()) {
        return Err(format!("an image is not the {} it says", image.media_type));
    }
    Ok(bytes)
}

/// Where the images people send are kept: one directory per person, by the SHA-256 of their ID,
/// each image a file named by its own SHA-256.
#[derive(Debug)]
pub(crate) struct ImageStore {
    dir: PathBuf,
}

impl ImageStore {
    /// The store in `dir`, made private (`0700` on Unix).
    ///
    /// # Errors
    ///
    /// The directory cannot be made.
    pub(crate) fn open(dir: PathBuf) -> io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self { dir })
    }

    fn person_dir(&self, person: &str) -> PathBuf {
        self.dir.join(sha256_hex(person.as_bytes()))
    }

    /// Keeps `bytes`, an image of `media_type` `person` sent; answers how it is named.
    ///
    /// # Errors
    ///
    /// It could not be written.
    pub(crate) fn put(
        &self,
        person: &str,
        media_type: &str,
        bytes: &[u8],
    ) -> io::Result<KeptImage> {
        let sha256 = sha256_hex(bytes);
        let dir = self.person_dir(person);
        std::fs::create_dir_all(&dir)?;
        let at = dir.join(&sha256);
        if !at.exists() {
            let temp = dir.join(format!("{sha256}.tmp-{}", uuid::Uuid::new_v4().simple()));
            std::fs::write(&temp, bytes)?;
            std::fs::rename(&temp, &at).inspect_err(|_| {
                let _ = std::fs::remove_file(&temp);
            })?;
        }
        Ok(KeptImage {
            sha256,
            media_type: media_type.to_owned(),
            bytes: bytes.len() as u64,
        })
    }

    /// The image `person` sent with SHA-256 `sha256`, with its format; `None` when they sent no
    /// such image or it is no longer kept.
    pub(crate) fn get(&self, person: &str, sha256: &str) -> Option<(&'static str, Vec<u8>)> {
        if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        let bytes = std::fs::read(self.person_dir(person).join(sha256)).ok()?;
        Some((sniff(&bytes)?, bytes))
    }

    /// Deletes the images last written longer than `age` ago, as her conversation log forgets
    /// the messages that named them.
    pub(crate) fn sweep(&self, age: Duration) {
        let Ok(people) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for person in people.flatten() {
            let Ok(images) = std::fs::read_dir(person.path()) else {
                continue;
            };
            for image in images.flatten() {
                let old = image
                    .metadata()
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|modified| modified.elapsed().ok())
                    .is_some_and(|elapsed| elapsed > age);
                if old {
                    let _ = std::fs::remove_file(image.path());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

    fn app(media_type: &str, bytes: &[u8]) -> AppImage {
        AppImage {
            media_type: media_type.to_owned(),
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }

    #[test]
    fn an_image_is_one_of_four_formats_by_its_bytes_and_within_its_bound() {
        assert_eq!(
            check(&app("image/png", PNG), ai6::Limit::at(64)).unwrap(),
            PNG
        );
        assert!(check(&app("image/svg+xml", b"<svg/>"), ai6::Limit::at(64)).is_err());
        assert!(check(&app("image/jpeg", PNG), ai6::Limit::at(64)).is_err());
        assert!(check(&app("image/png", PNG), ai6::Limit::at(4)).is_err());
        let not_base64 = AppImage {
            media_type: "image/png".to_owned(),
            data: "not base64!".to_owned(),
        };
        assert!(check(&not_base64, ai6::Limit::at(64)).is_err());
    }

    #[test]
    fn an_image_is_kept_for_its_person_alone() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImageStore::open(dir.path().join("images")).unwrap();
        let kept = store.put("ana", "image/png", PNG).unwrap();
        assert_eq!(kept.sha256, sha256_hex(PNG));
        assert_eq!(store.get("ana", &kept.sha256).unwrap().1, PNG);
        assert!(store.get("bob", &kept.sha256).is_none());
        assert!(store.get("ana", "../ana").is_none());
        store.sweep(Duration::from_secs(3600));
        assert!(store.get("ana", &kept.sha256).is_some());
        std::thread::sleep(Duration::from_millis(20));
        store.sweep(Duration::from_millis(1));
        assert!(store.get("ana", &kept.sha256).is_none());
    }
}
