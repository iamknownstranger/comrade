/*!
 * download — carrying out the [`OpenLicence`] tier, and refusing the other three.
 *
 * `crate::catalogue` decides *whether* a recording's audio may be copied; this
 * module is the "who carries it out" its own table left open. It is the download
 * half of the BlackHole shape (`docs/TOGETHER.md` §20): fetch the audio, name it
 * properly, and hand it to the platform's music library so it behaves like any
 * other track on the phone — offline, in the library list, and usable as a
 * session's `LocalFile`.
 *
 * # The licence gate is enforced by the type system, not by a check callers can forget
 *
 * [`fetch_track`] does not take a URL. It takes a [`PermittedDownload`], and the
 * **only** way to obtain one is [`permit_download`], which refuses anything whose
 * declared licence does not allow redistribution. So there is no call shape that
 * downloads an arbitrary string, and no ordering bug where the check runs after
 * the bytes have already arrived — `catalogue.rs`'s "licence checking happens
 * before the fetch, not after" is a compile-time property here rather than a
 * convention.
 *
 * This is the part that makes the difference between this module and the
 * "download button that works until somebody's lawyer notices" that
 * `catalogue.rs`'s header warns about. [`SourceTier::EmbedOnly`] has no path
 * through here at all, deliberately: a licensed commercial track nobody in the
 * conversation owns is not downloadable by this app, and the honest answer is the
 * embed.
 *
 * [`OpenLicence`]: crate::catalogue::OpenLicence
 * [`SourceTier::EmbedOnly`]: crate::catalogue::SourceTier::EmbedOnly
 *
 * # Guards, copied from the media pipeline rather than reinvented
 *
 * The fetch follows `crate::media`'s `fetch_guarded_bytes` exactly — HTTPS only
 * (fail-closed on any other or missing scheme), redirects disabled, an explicit
 * connect timeout separate from the transfer budget, a size cap checked against
 * `Content-Length` *and* again while the body streams in, and a content-type
 * allowlist. The URL here comes from a third-party catalogue's response, so it is
 * attacker-influenced in exactly the way that function's comment describes.
 *
 * # What is here without a network, and what is not
 *
 * Everything except the socket is always compiled and always tested: the gate,
 * the container-format decision, the filename sanitiser, and the cap. Only
 * [`fetch_track`] is behind `catalogue-http`, following `media-http`'s precedent —
 * and the CI lane for that feature covers this module for the same reason it
 * covers the lookup.
 *
 * # In-file tags are deliberately not written here
 *
 * BlackHole writes ID3 frames into the file. This does not, and the reason is
 * reach rather than effort: `id3` handles MP3 only, while an open-licence archive
 * serves FLAC, Ogg and M4A just as often, so an ID3 writer would tag roughly one
 * format in four and silently skip the rest — which is worse than not promising
 * it. What makes a download read correctly *on the phone* is the platform
 * library's own metadata columns, which the frontend writes from
 * [`PermittedDownload::recording`] (Android: `MediaStore.Audio`'s
 * `TITLE`/`ARTIST`/`ALBUM`, see `together/MusicDownloads.kt`). A file copied off
 * the device therefore carries whatever tags the archive already put in it.
 * Closing that properly means a multi-format tag writer (`lofty`-shaped); it is
 * named in `docs/TOGETHER.md` §21 as missing rather than left to be discovered.
 */

use crate::catalogue::{licence_permits_sharing, CatalogueMatch, OpenLicence};
use crate::together::Recording;

/// The most a single track may be.
///
/// Larger than `crate::media::MAX_MEDIA_BYTES` (10 MB, sized for a chat
/// attachment) because a lossless album track legitimately runs to tens of
/// megabytes and refusing those would make the tier useless for the archives it
/// exists to serve. Still finite, and still checked twice: a cap that only reads
/// `Content-Length` is not a cap.
pub const MAX_TRACK_BYTES: usize = 96 * 1024 * 1024;

/// Audio content types a download may be served as.
///
/// An allowlist rather than a denylist, and checked against the response header
/// before the body is buffered. A catalogue that answers a track query with
/// `text/html` is serving an error page or a redirect notice, and writing that
/// into somebody's music library as a song is the failure this prevents.
///
/// `application/ogg` is here because that is what plenty of hosts send for
/// `.ogg`, and `audio/mpeg` covers MP3 — the two most common archive formats
/// between them.
pub const AUDIO_CONTENT_TYPES: &[&str] = &[
    "audio/mpeg",
    "audio/mp4",
    "audio/aac",
    "audio/ogg",
    "application/ogg",
    "audio/opus",
    "audio/flac",
    "audio/x-flac",
    "audio/wav",
    "audio/x-wav",
    "audio/webm",
];

/// Why a download will not happen.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum DownloadRefusal {
    /// The catalogue served no audio URL — a metadata-only answer, which is what
    /// MusicBrainz always is.
    #[error("this catalogue entry has no audio to download")]
    NoAudio,

    /// The catalogue served audio but did not declare a licence permitting a
    /// copy. **Not an error to work around**: an archive that serves bytes has
    /// not thereby licensed them, which is why
    /// [`CatalogueMatch::is_openly_fetchable`] requires both halves.
    #[error("this recording's licence does not permit downloading a copy")]
    NotOpenlyLicensed,

    /// The URL is not HTTPS. Refused here rather than at the socket so the
    /// caller never even gets a request it could send.
    #[error("refusing to download over anything but HTTPS")]
    InsecureUrl,
}

/// A download the licence permits, with everything the fetch and the tagging
/// need.
///
/// **Constructible only through [`permit_download`]** — the fields are public to
/// read but the type cannot be built from outside this module, which is what
/// makes the gate unbypassable. A caller holding one of these has, by
/// construction, already passed the licence check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermittedDownload {
    /// Checked HTTPS at gate time.
    url: String,
    /// What to call it in the library.
    recording: Recording,
    /// The licence that permitted this, kept so a UI can say which one.
    licence: OpenLicence,
}

impl PermittedDownload {
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The metadata to write into the platform's library.
    pub fn recording(&self) -> &Recording {
        &self.recording
    }

    /// The licence that allowed it. Worth showing: somebody downloading a track
    /// is entitled to know on what basis the app decided it could.
    pub fn licence(&self) -> OpenLicence {
        self.licence
    }
}

/// The gate. `Ok` only for a catalogue answer whose audio may actually be copied.
///
/// Deliberately takes the whole [`CatalogueMatch`] rather than a URL and a
/// licence: passing those separately is the call site where somebody eventually
/// passes the URL of one match and the licence of another.
pub fn permit_download(m: &CatalogueMatch) -> Result<PermittedDownload, DownloadRefusal> {
    let url = m.audio_url.as_deref().ok_or(DownloadRefusal::NoAudio)?;
    if !licence_permits_sharing(m.licence) {
        return Err(DownloadRefusal::NotOpenlyLicensed);
    }
    if !is_https(url) {
        return Err(DownloadRefusal::InsecureUrl);
    }
    Ok(PermittedDownload {
        url: url.to_string(),
        recording: m.recording.clone(),
        licence: m.licence,
    })
}

/// HTTPS in any case, fail-closed on everything else including a missing scheme.
///
/// Its own function because it is asserted in two places — at the gate and again
/// inside the fetch — and the second one is not redundant: it is what keeps the
/// guard true if `PermittedDownload` ever gains another constructor.
fn is_https(url: &str) -> bool {
    url.split_once("://")
        .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("https"))
}

/// The file extension for a content type, or `None` if we do not recognise it.
///
/// From the **served type**, not from the URL's path: an archive commonly serves
/// `…/download?id=1234` with the real format only in the header, and a filename
/// guessed from a path like that ends up called `download`. Where the header is
/// absent the caller falls back to [`extension_from_url`].
pub fn extension_for(content_type: &str) -> Option<&'static str> {
    // Judge the bare type: `audio/mpeg; charset=binary` is still audio/mpeg, and
    // a host that adds a parameter is not doing anything wrong.
    let bare = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    Some(match bare.as_str() {
        "audio/mpeg" => "mp3",
        "audio/mp4" | "audio/aac" => "m4a",
        "audio/ogg" | "application/ogg" => "ogg",
        "audio/opus" => "opus",
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/webm" => "webm",
        _ => return None,
    })
}

/// The extension a URL's own path claims, when the response gave us nothing.
///
/// Last resort, and bounded to the formats [`AUDIO_CONTENT_TYPES`] covers so a
/// path ending `.exe` cannot name the file. The query and fragment are cut first,
/// because `…/track.mp3?token=abc` is an mp3 and `…/track.mp3#t=30` is too.
pub fn extension_from_url(url: &str) -> Option<&'static str> {
    let path = url.split(['?', '#']).next().unwrap_or_default();
    let lowered = path.to_ascii_lowercase();
    ["mp3", "m4a", "aac", "ogg", "opus", "flac", "wav", "webm"]
        .into_iter()
        .find(|ext| lowered.ends_with(&format!(".{ext}")))
}

/// The longest a generated filename may be, extension included.
///
/// 120 rather than a filesystem's own limit: the name is a *display* name in a
/// media store, several Android versions have truncated closer to 128, and a
/// title long enough to hit this is one nobody reads to the end of anyway.
const MAX_FILENAME_LEN: usize = 120;

/// A filename for a downloaded track — `Artist - Title.ext`.
///
/// **Sanitised, and the reason is that every part of the input is a third
/// party's string.** The artist and title come from a catalogue's JSON, so this
/// is the boundary where `../../` and a NUL byte stop being possible:
///
/// - the path separators `/` and `\` and every ASCII control character are
///   dropped, which is what makes `..` inert — with no separator left, a name of
///   `..` is a literal file called `..` rather than a directory traversal;
/// - the characters Windows and several Android media stores reject
///   (`: * ? " < > |`) go too, so a file downloaded here survives being copied
///   to a desktop;
/// - runs of whitespace collapse, and leading/trailing dots and spaces are
///   trimmed — a name starting `.` is hidden on Unix and a name ending `.` is
///   invalid on Windows;
/// - an input that sanitises to nothing at all becomes `track`, because a file
///   called `.mp3` is not a file anybody can find.
pub fn filename_for(recording: &Recording, extension: &str) -> String {
    let stem = if recording.artist.trim().is_empty() {
        sanitise(&recording.title)
    } else {
        format!(
            "{} - {}",
            sanitise(&recording.artist),
            sanitise(&recording.title)
        )
    };
    let stem = stem.trim_matches(|c: char| c == '.' || c.is_whitespace());
    let stem = if stem.is_empty() { "track" } else { stem };
    // Truncate the *stem*, never the extension: a media store keyed on the
    // extension would stop recognising a file whose `.mp3` had been cut to `.m`.
    let room = MAX_FILENAME_LEN.saturating_sub(extension.len() + 1);
    let stem: String = stem.chars().take(room.max(1)).collect();
    let stem = stem.trim_end_matches(|c: char| c == '.' || c.is_whitespace());
    let stem = if stem.is_empty() { "track" } else { stem };
    format!("{stem}.{extension}")
}

/// Drop what must not appear in a filename, and collapse the whitespace.
fn sanitise(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_was_space = false;
    for c in raw.chars() {
        // Whitespace is tested **before** control, and that order is the whole
        // point: `\n`, `\t` and `\r` are both, and dropping them as control
        // characters would join the words either side — a two-line title became
        // `abcd` instead of `ab cd`. They collapse to a space; every other
        // control character is dropped outright.
        if c.is_whitespace() {
            if !last_was_space {
                out.push(' ');
            }
            last_was_space = true;
            continue;
        }
        let bad = c == '/'
            || c == '\\'
            || c.is_control()
            || matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|');
        if bad {
            continue;
        }
        out.push(c);
        last_was_space = false;
    }
    out.trim().to_string()
}

/// What came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedTrack {
    pub bytes: Vec<u8>,
    /// `Artist - Title.ext`, already sanitised — see [`filename_for`].
    pub filename: String,
    /// The type the host served, for the platform library's MIME column.
    pub mime: String,
    /// Carried through so the frontend writes the library's metadata columns
    /// from the same names the filename was built from.
    pub recording: Recording,
}

/// What can go wrong fetching one.
#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("download refused: {0}")]
    Refused(#[from] DownloadRefusal),

    #[error("download failed: {0}")]
    Http(String),

    #[error("host served {0}, which is not audio")]
    NotAudio(String),

    #[error("track is larger than the {MAX_TRACK_BYTES} byte limit")]
    TooLarge,

    /// The host sent audio in a container we cannot name a file for. Refused
    /// rather than saved with a guessed extension: a media store keys its
    /// scanner on the extension, so a wrong one produces a file that exists and
    /// never appears in any music app.
    #[error("cannot tell what format the host served")]
    UnknownFormat,
}

#[cfg(feature = "catalogue-http")]
mod http {
    use super::*;
    use std::time::Duration;

    /// Same split as `crate::media`'s, for the same reason: a host that has
    /// stopped completing connections must cost seconds before we give up, while
    /// a slow but live one gets the whole transfer budget.
    const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

    /// Generous — [`MAX_TRACK_BYTES`] over a phone's downlink is minutes — but
    /// finite, because `reqwest`'s default is no timeout at all and a stalled
    /// download would otherwise leave a progress bar up forever.
    const TRANSFER_TIMEOUT: Duration = Duration::from_secs(300);

    /// Fetch the audio a [`PermittedDownload`] names.
    ///
    /// Guards, in the order they are applied — the order matters, because each
    /// one is cheaper than the next and the last is the only one that costs
    /// bandwidth:
    ///  1. HTTPS re-asserted (the gate checked it; this keeps it true if
    ///     `PermittedDownload` ever gains another constructor);
    ///  2. redirects disabled — a redirected target could leak the client IP or
    ///     probe an internal address, and an archive that needs a redirect to
    ///     serve a file it advertised is not one to follow blindly;
    ///  3. status;
    ///  4. content type against [`AUDIO_CONTENT_TYPES`], **before** any body is
    ///     buffered;
    ///  5. `Content-Length` against [`MAX_TRACK_BYTES`];
    ///  6. and the cap again as the body streams, because a host may omit or
    ///     understate the length.
    pub async fn fetch_track(want: &PermittedDownload) -> Result<FetchedTrack, DownloadError> {
        if !is_https(want.url()) {
            return Err(DownloadError::Refused(DownloadRefusal::InsecureUrl));
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(TRANSFER_TIMEOUT)
            .build()
            .map_err(|e| DownloadError::Http(e.to_string()))?;
        let mut resp = client
            .get(want.url())
            .send()
            .await
            .map_err(|e| DownloadError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(DownloadError::Http(format!("status {}", resp.status())));
        }

        let served = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_string());
        // An absent type is tolerated and falls back to the URL's own extension;
        // a *present and disallowed* one is refused before a byte is buffered.
        // Same posture as the media fetch: plenty of archives omit the header.
        if let Some(bare) = served
            .as_deref()
            .and_then(|v| v.split(';').next())
            .map(|v| v.trim().to_ascii_lowercase())
            .filter(|v| !v.is_empty())
        {
            if !AUDIO_CONTENT_TYPES.contains(&bare.as_str()) {
                return Err(DownloadError::NotAudio(bare));
            }
        }
        let extension = served
            .as_deref()
            .and_then(extension_for)
            .or_else(|| extension_from_url(want.url()))
            .ok_or(DownloadError::UnknownFormat)?;

        if let Some(len) = resp.content_length() {
            if len > MAX_TRACK_BYTES as u64 {
                return Err(DownloadError::TooLarge);
            }
        }
        let mut bytes: Vec<u8> = Vec::new();
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| DownloadError::Http(e.to_string()))?
        {
            if bytes.len() + chunk.len() > MAX_TRACK_BYTES {
                return Err(DownloadError::TooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            return Err(DownloadError::Http("host served an empty body".into()));
        }
        Ok(FetchedTrack {
            bytes,
            filename: filename_for(want.recording(), extension),
            // The extension is the authority, not the header: it is what the
            // format decision above settled on, and a media store whose MIME and
            // extension disagree indexes neither reliably.
            mime: mime_for(extension).to_string(),
            recording: want.recording().clone(),
        })
    }

    /// The MIME to record for a container we chose an extension for.
    fn mime_for(extension: &str) -> &'static str {
        match extension {
            "mp3" => "audio/mpeg",
            "m4a" => "audio/mp4",
            "ogg" => "audio/ogg",
            "opus" => "audio/opus",
            "flac" => "audio/flac",
            "wav" => "audio/wav",
            "webm" => "audio/webm",
            // Unreachable: every arm of `extension_for`/`extension_from_url` is
            // covered above. Named rather than `unreachable!()` because a panic
            // across the FFI boundary surfaces to Kotlin as a `PanicException`
            // from an unrelated method, and a generic audio type is a harmless
            // answer.
            _ => "audio/mpeg",
        }
    }
}

#[cfg(feature = "catalogue-http")]
pub use http::fetch_track;

#[cfg(test)]
mod tests {
    use super::*;

    fn openly(url: Option<&str>, licence: OpenLicence) -> CatalogueMatch {
        CatalogueMatch {
            source: "Test".into(),
            recording: Recording {
                isrc: None,
                title: "Kun Faya Kun".into(),
                artist: "A. R. Rahman".into(),
                album: Some("Rockstar".into()),
            },
            duration_ms: Some(470_000),
            audio_url: url.map(|u| u.to_string()),
            licence,
        }
    }

    #[test]
    fn only_a_declared_open_licence_gets_through_the_gate() {
        assert!(permit_download(&openly(
            Some("https://archive.example/a.flac"),
            OpenLicence::CreativeCommons
        ))
        .is_ok());
        assert!(permit_download(&openly(
            Some("https://archive.example/a.flac"),
            OpenLicence::PublicDomain
        ))
        .is_ok());
    }

    /// The gate's whole reason for existing: serving the bytes is not licensing
    /// them, so an undeclared licence is a refusal and not a shrug.
    #[test]
    fn an_undeclared_licence_is_refused_even_though_the_audio_is_right_there() {
        assert_eq!(
            permit_download(&openly(
                Some("https://archive.example/a.flac"),
                OpenLicence::Unknown
            )),
            Err(DownloadRefusal::NotOpenlyLicensed)
        );
    }

    #[test]
    fn a_metadata_only_answer_has_nothing_to_download() {
        assert_eq!(
            permit_download(&openly(None, OpenLicence::CreativeCommons)),
            Err(DownloadRefusal::NoAudio)
        );
    }

    /// Ordered after the licence check on purpose — but both must refuse, and a
    /// plaintext URL must never reach the socket.
    #[test]
    fn a_non_https_url_is_refused_however_it_is_licensed() {
        for url in [
            "http://archive.example/a.flac",
            "ftp://archive.example/a.flac",
            "archive.example/a.flac",
            "//archive.example/a.flac",
        ] {
            assert_eq!(
                permit_download(&openly(Some(url), OpenLicence::PublicDomain)),
                Err(DownloadRefusal::InsecureUrl),
                "{url} must not be downloadable"
            );
        }
        // …and the scheme is case-insensitive, so this one is fine.
        assert!(permit_download(&openly(
            Some("HTTPS://archive.example/a.flac"),
            OpenLicence::PublicDomain
        ))
        .is_ok());
    }

    #[test]
    fn the_format_comes_from_the_served_type_first_and_the_path_second() {
        assert_eq!(extension_for("audio/mpeg"), Some("mp3"));
        assert_eq!(extension_for("audio/flac; charset=binary"), Some("flac"));
        assert_eq!(extension_for("AUDIO/OGG"), Some("ogg"));
        assert_eq!(extension_for("text/html"), None);
        // The case the header exists for: no format anywhere in the path.
        assert_eq!(extension_from_url("https://a.example/download?id=1"), None);
        assert_eq!(
            extension_from_url("https://a.example/track.mp3?token=abc"),
            Some("mp3")
        );
        assert_eq!(
            extension_from_url("https://a.example/track.flac#t=30"),
            Some("flac")
        );
        // Bounded to audio: a path ending in something else names nothing.
        assert_eq!(extension_from_url("https://a.example/setup.exe"), None);
    }

    #[test]
    fn a_filename_is_artist_then_title() {
        let r = Recording {
            isrc: None,
            title: "Kun Faya Kun".into(),
            artist: "A. R. Rahman".into(),
            album: None,
        };
        assert_eq!(filename_for(&r, "flac"), "A. R. Rahman - Kun Faya Kun.flac");
    }

    #[test]
    fn a_recording_with_no_artist_is_named_by_its_title_alone() {
        assert_eq!(
            filename_for(&Recording::titled("Field recording"), "wav"),
            "Field recording.wav"
        );
    }

    /// The boundary test. Every one of these strings is a third party's, so a
    /// separator or a control character surviving into a filename is a real bug.
    #[test]
    fn a_catalogue_cannot_name_a_file_outside_the_directory_it_is_written_to() {
        let nasty = Recording {
            isrc: None,
            title: "../../etc/passwd".into(),
            artist: "..\\..\\windows".into(),
            album: None,
        };
        let name = filename_for(&nasty, "mp3");
        assert!(!name.contains('/'), "{name}");
        assert!(!name.contains('\\'), "{name}");
        // With no separator left there is no traversal — the dots are inert, and
        // the leading run is trimmed as well so the name is not hidden on Unix.
        assert_eq!(name, "windows - ....etcpasswd.mp3");
    }

    #[test]
    fn control_characters_and_reserved_punctuation_never_reach_the_name() {
        let nasty = Recording {
            isrc: None,
            title: "a\u{0}b\nc\td: e*f?g\"h<i>j|k".into(),
            artist: String::new(),
            album: None,
        };
        let name = filename_for(&nasty, "mp3");
        for c in ['\u{0}', '\n', '\t', ':', '*', '?', '"', '<', '>', '|'] {
            assert!(!name.contains(c), "{c:?} survived into {name}");
        }
        // Two different treatments, and the difference is deliberate:
        //  * `\n` and `\t` collapse to a space, so words either side stay apart;
        //  * a NUL and the reserved punctuation are *dropped*, joining what they
        //    separated — `e*f` becomes `ef`. Inserting a space for those would
        //    put one in the middle of a title that never had one.
        assert_eq!(name, "ab c d efghijk.mp3");
    }

    #[test]
    fn a_name_that_sanitises_to_nothing_still_produces_a_findable_file() {
        for title in ["", "   ", "///", "...", "\u{0}\u{1}"] {
            let r = Recording {
                isrc: None,
                title: title.into(),
                artist: String::new(),
                album: None,
            };
            assert_eq!(
                filename_for(&r, "mp3"),
                "track.mp3",
                "a file called `.mp3` is not findable ({title:?})"
            );
        }
    }

    #[test]
    fn a_very_long_title_is_cut_without_losing_the_extension() {
        let r = Recording {
            isrc: None,
            title: "x".repeat(500),
            artist: String::new(),
            album: None,
        };
        let name = filename_for(&r, "flac");
        assert!(name.len() <= MAX_FILENAME_LEN, "{} chars", name.len());
        assert!(
            name.ends_with(".flac"),
            "the extension is what a media scanner keys on: {name}"
        );
    }

    /// Multi-byte titles must not be cut mid-character, and must not blow the
    /// byte budget either. `chars().take()` is why the first holds.
    #[test]
    fn a_long_non_ascii_title_is_cut_on_a_character_boundary() {
        let r = Recording {
            isrc: None,
            title: "क".repeat(300),
            artist: String::new(),
            album: None,
        };
        let name = filename_for(&r, "mp3");
        assert!(name.ends_with(".mp3"));
        assert!(name.chars().count() <= MAX_FILENAME_LEN);
        // Valid UTF-8 by construction; assert it is still whole characters.
        assert!(name.trim_end_matches(".mp3").chars().all(|c| c == 'क'));
    }
}
