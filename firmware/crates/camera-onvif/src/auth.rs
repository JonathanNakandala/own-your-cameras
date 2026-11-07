//! WS-Security UsernameToken PasswordDigest verification: `Base64(SHA1(nonce + created + password))`,
//! with a ±5 minute window on `Created` and a bounded cache that refuses a nonce seen in that window.
//! Pure: the time is passed in.

use camera_transport::Credentials;
use std::collections::VecDeque;
use std::time::{Duration, SystemTime};

/// How far `Created` may be from the camera clock, either way.
pub const WINDOW: Duration = Duration::from_secs(5 * 60);
/// Remembered nonces; a full cache forgets the oldest.
const NONCES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsernameToken {
    pub username: String,
    /// `None` for anything but `#PasswordDigest` (plain-text passwords are refused).
    pub digest: Option<Vec<u8>>,
    pub nonce: Vec<u8>,
    pub created: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    /// Not a PasswordDigest token, or its nonce or timestamp is missing or malformed.
    Unsupported,
    WrongCredentials,
    /// `Created` is more than [`WINDOW`] from the camera clock.
    Stale,
    Replay,
}

#[derive(Default)]
pub struct NonceCache {
    seen: VecDeque<(SystemTime, Vec<u8>)>,
    /// `Created` of the newest nonce ever evicted: older tokens could be replays of forgotten nonces.
    floor: Option<SystemTime>,
}

impl NonceCache {
    fn prune(&mut self, now: SystemTime) {
        self.seen.retain(|(created, _)| {
            now.duration_since(*created).unwrap_or_default() <= WINDOW
                && created.duration_since(now).unwrap_or_default() <= WINDOW
        });
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

pub fn digest(nonce: &[u8], created: &str, password: &str) -> [u8; 20] {
    let mut input = nonce.to_vec();
    input.extend_from_slice(created.as_bytes());
    input.extend_from_slice(password.as_bytes());
    crate::codec::sha1(&input)
}

pub fn verify(
    token: &UsernameToken,
    creds: &Credentials,
    now: SystemTime,
    seen: &mut NonceCache,
) -> Result<(), AuthError> {
    let given = token.digest.as_deref().ok_or(AuthError::Unsupported)?;
    if token.nonce.is_empty() {
        return Err(AuthError::Unsupported);
    }
    let created = crate::time::parse(&token.created).ok_or(AuthError::Unsupported)?;
    let expected = digest(&token.nonce, &token.created, &creds.password);
    let user_ok = constant_time_eq(token.username.as_bytes(), creds.user.as_bytes());
    if !(user_ok & constant_time_eq(given, &expected)) {
        return Err(AuthError::WrongCredentials);
    }
    let skew = now
        .duration_since(created)
        .or_else(|e| Ok::<_, ()>(e.duration()))
        .unwrap_or_default();
    if skew > WINDOW {
        return Err(AuthError::Stale);
    }
    seen.prune(now);
    if seen.seen.iter().any(|(_, n)| *n == token.nonce) || seen.floor.is_some_and(|f| created <= f)
    {
        return Err(AuthError::Replay);
    }
    if seen.seen.len() >= NONCES
        && let Some((evicted, _)) = seen.seen.pop_front()
    {
        seen.floor = Some(seen.floor.map_or(evicted, |f| f.max(evicted)));
    }
    seen.seen.push_back((created, token.nonce.clone()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{base64_decode, base64_encode};

    fn creds() -> Credentials {
        Credentials {
            user: "admin".into(),
            password: "userpassword".into(),
        }
    }

    fn token(nonce: &str, created: &str, password: &str) -> UsernameToken {
        let nonce = base64_decode(nonce).unwrap();
        UsernameToken {
            username: "admin".into(),
            digest: Some(digest(&nonce, created, password).to_vec()),
            nonce,
            created: created.into(),
        }
    }

    #[test]
    fn the_onvif_programmers_guide_example_digest() {
        // ONVIF Application Programmer's Guide, "Authentication": nonce, created and password give this
        // PasswordDigest.
        let nonce = base64_decode("LKqI6G/AikKCQrN0zqZFlg==").unwrap();
        assert_eq!(
            base64_encode(&digest(&nonce, "2010-09-16T07:50:45Z", "userpassword")),
            "tuOSpGlFlIXsozq4HFNeeGeFLEI="
        );
    }

    #[test]
    fn a_fresh_digest_passes_once_and_its_replay_is_refused() {
        let created = "2026-11-07T12:00:00Z";
        let now = crate::time::parse(created).unwrap() + Duration::from_secs(30);
        let mut seen = NonceCache::default();
        let t = token("LKqI6G/AikKCQrN0zqZFlg==", created, "userpassword");
        assert_eq!(verify(&t, &creds(), now, &mut seen), Ok(()));
        assert_eq!(verify(&t, &creds(), now, &mut seen), Err(AuthError::Replay));
        let other = token("AAAAAAAAAAAAAAAAAAAAAA==", created, "userpassword");
        assert_eq!(verify(&other, &creds(), now, &mut seen), Ok(()));
    }

    #[test]
    fn a_full_cache_refuses_tokens_no_newer_than_the_last_forgotten_nonce() {
        let base = crate::time::parse("2026-11-07T12:00:00Z").unwrap();
        let mut seen = NonceCache::default();
        let mut first = None;
        for i in 0..=NONCES as u64 {
            let created = crate::time::format(base + Duration::from_millis(i * 100));
            let nonce = crate::codec::base64_encode(&i.to_be_bytes());
            let t = token(&nonce, &created, "userpassword");
            first.get_or_insert_with(|| t.clone());
            assert_eq!(
                verify(&t, &creds(), base + Duration::from_secs(60), &mut seen),
                Ok(())
            );
        }
        // The first nonce was forgotten to make room; its replay is still refused.
        let replay = first.unwrap();
        assert_eq!(
            verify(&replay, &creds(), base + Duration::from_secs(60), &mut seen),
            Err(AuthError::Replay)
        );
    }

    #[test]
    fn wrong_passwords_users_stale_times_and_plain_text_are_refused() {
        let created = "2026-11-07T12:00:00Z";
        let at = crate::time::parse(created).unwrap();
        let mut seen = NonceCache::default();
        let wrong = token("LKqI6G/AikKCQrN0zqZFlg==", created, "nope");
        assert_eq!(
            verify(&wrong, &creds(), at, &mut seen),
            Err(AuthError::WrongCredentials)
        );
        let mut user = token("LKqI6G/AikKCQrN0zqZFlg==", created, "userpassword");
        user.username = "guest".into();
        assert_eq!(
            verify(&user, &creds(), at, &mut seen),
            Err(AuthError::WrongCredentials)
        );
        let t = token("LKqI6G/AikKCQrN0zqZFlg==", created, "userpassword");
        for off in [-301i64, 301] {
            let now = if off < 0 {
                at - Duration::from_secs(301)
            } else {
                at + Duration::from_secs(301)
            };
            assert_eq!(verify(&t, &creds(), now, &mut seen), Err(AuthError::Stale));
        }
        let mut plain = t.clone();
        plain.digest = None;
        assert_eq!(
            verify(&plain, &creds(), at, &mut seen),
            Err(AuthError::Unsupported)
        );
        let mut no_nonce = t;
        no_nonce.nonce.clear();
        assert_eq!(
            verify(&no_nonce, &creds(), at, &mut seen),
            Err(AuthError::Unsupported)
        );
    }
}
