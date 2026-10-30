//! Browser push encryption with a fresh sender key and salt.
//!
//! ECE handles the RFC 8291 key agreement, HKDF and AES-128-GCM record. Browser
//! keys are checked before calling it, and only one bounded record is sent.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use orch_notifier_contract::delivery::DeliverError;

/// RFC 8291 requires a 65-byte uncompressed P-256 point and a 16-byte secret.
const PUBLIC_KEY_BYTES: usize = 65;
const AUTH_SECRET_BYTES: usize = 16;
/// Push services need only accept 4096 bytes, including the 86-byte header
/// and 16-byte tag. ECE pads to a multiple of 128 bytes, so 3967 is the largest
/// plaintext whose padded record fits that body and remains a single record.
const MAX_PLAINTEXT_BYTES: usize = 3967;
const MAX_BODY_BYTES: usize = 4096;

fn bad_keys() -> DeliverError {
    DeliverError::gone(
        "bad_subscription",
        "the browser's encryption keys cannot be used",
    )
}

/// Seals one bounded record for the validated keys of a browser subscription.
pub(crate) fn seal(p256dh: &str, auth: &str, content: &[u8]) -> Result<Vec<u8>, DeliverError> {
    let public = URL_SAFE_NO_PAD.decode(p256dh).map_err(|_| bad_keys())?;
    let auth = URL_SAFE_NO_PAD.decode(auth).map_err(|_| bad_keys())?;
    if public.len() != PUBLIC_KEY_BYTES
        || auth.len() != AUTH_SECRET_BYTES
        || p256::PublicKey::from_sec1_bytes(&public).is_err()
    {
        return Err(bad_keys());
    }
    if content.len() > MAX_PLAINTEXT_BYTES {
        return Err(DeliverError::refused(
            "message",
            "the push message is too large",
        ));
    }
    let body = ece::encrypt(&public, &auth, content)
        .map_err(|_| DeliverError::refused("message", "the push message could not be encrypted"))?;
    if body.len() > MAX_BODY_BYTES {
        return Err(DeliverError::refused(
            "message",
            "the push message is too large",
        ));
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The message and receiver from RFC 8291's example, with its complete
    // aes128gcm body. Its sender key and salt are intentionally fixed there.
    const PLAIN: &[u8] = b"When I grow up, I want to be a watermelon";
    const PRIVATE: &str = "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94";
    const PUBLIC: &str = concat!(
        "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcx",
        "aOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4"
    );
    const AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";
    const CIPHERTEXT: &str = concat!(
        "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27ml",
        "mlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPT",
        "pK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN"
    );

    fn receiver() -> ece::EcKeyComponents {
        ece::EcKeyComponents::new(
            URL_SAFE_NO_PAD.decode(PRIVATE).unwrap(),
            URL_SAFE_NO_PAD.decode(PUBLIC).unwrap(),
        )
    }

    #[test]
    fn the_standards_example_opens_and_the_sender_seals_it_for_the_same_browser() {
        let auth = URL_SAFE_NO_PAD.decode(AUTH).unwrap();
        let ciphertext = URL_SAFE_NO_PAD.decode(CIPHERTEXT).unwrap();
        assert_eq!(
            ece::decrypt(&receiver(), &auth, &ciphertext).unwrap(),
            PLAIN
        );
        let first = seal(PUBLIC, AUTH, PLAIN).unwrap();
        let second = seal(PUBLIC, AUTH, PLAIN).unwrap();
        assert_eq!(ece::decrypt(&receiver(), &auth, &first).unwrap(), PLAIN);
        assert_eq!(ece::decrypt(&receiver(), &auth, &second).unwrap(), PLAIN);
        assert_ne!(&first[..16], &second[..16], "each message has a fresh salt");
        assert_ne!(
            &first[21..86],
            &second[21..86],
            "each message has a fresh sender key"
        );
        let mut changed = first;
        let last = changed.len() - 1;
        changed[last] ^= 1;
        assert!(ece::decrypt(&receiver(), &auth, &changed).is_err());
        let mut wrong_auth = auth;
        wrong_auth[0] ^= 1;
        assert!(ece::decrypt(&receiver(), &wrong_auth, &second).is_err());
    }

    #[test]
    fn invalid_browser_keys_are_gone_while_a_message_too_large_keeps_the_subscription() {
        for (public, auth) in [
            ("garbage", AUTH),
            (PUBLIC, "garbage"),
            (PUBLIC, "AAAA"),
            ("AAAA", AUTH),
        ] {
            assert!(seal(public, auth, PLAIN).unwrap_err().gone);
        }
        let mut off_curve = [0; PUBLIC_KEY_BYTES];
        off_curve[0] = 4;
        assert!(
            seal(&URL_SAFE_NO_PAD.encode(off_curve), AUTH, PLAIN)
                .unwrap_err()
                .gone
        );
        let auth = URL_SAFE_NO_PAD.decode(AUTH).unwrap();
        let maximum = vec![b'x'; MAX_PLAINTEXT_BYTES];
        let sealed = seal(PUBLIC, AUTH, &maximum).unwrap();
        assert!(sealed.len() <= MAX_BODY_BYTES);
        assert_eq!(ece::decrypt(&receiver(), &auth, &sealed).unwrap(), maximum);
        let error = seal(PUBLIC, AUTH, &vec![b'x'; MAX_PLAINTEXT_BYTES + 1]).unwrap_err();
        assert!(!error.gone && !error.retryable);
    }
}
