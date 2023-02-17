#[cfg(test)]
mod tests {
    use bitcoin_hashes::hex::ToHex;
    use bitcoin_hashes::sha256;
    use bitcoin_hashes::Hash;
    use secp256k1::rand;
    use secp256k1::{KeyPair, Secp256k1, XOnlyPublicKey};

    use nostr_rs_relay::conn::ClientConn;
    use nostr_rs_relay::error::Error;
    use nostr_rs_relay::event::Event;
    use nostr_rs_relay::utils::unix_time;

    const RELAY: &str = "wss://nostr.example.com/";

    #[test]
    fn test_generate_auth_challenge() {
        let mut client_conn = ClientConn::new("127.0.0.1".into());

        assert_eq!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), None);

        client_conn.generate_auth_challenge();

        assert_ne!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), None);

        let last_auth_challenge = client_conn.auth_challenge().cloned();

        client_conn.generate_auth_challenge();

        assert_ne!(client_conn.auth_challenge(), None);
        assert_ne!(
            client_conn.auth_challenge().unwrap(),
            &last_auth_challenge.unwrap()
        );
        assert_eq!(client_conn.auth_pubkey(), None);
    }

    #[test]
    fn test_authenticate_with_valid_event() {
        let mut client_conn = ClientConn::new("127.0.0.1".into());

        assert_eq!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), None);

        client_conn.generate_auth_challenge();

        assert_ne!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), None);

        let challenge = client_conn.auth_challenge().unwrap();
        let event = auth_event(challenge);

        let result = client_conn.authenticate(&event, &RELAY.into());

        assert!(matches!(result, Ok(())));
        assert_eq!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), Some(&event.pubkey));
    }

    #[test]
    fn test_fail_to_authenticate_in_invalid_state() {
        let mut client_conn = ClientConn::new("127.0.0.1".into());

        assert_eq!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), None);

        let event = auth_event(&"challenge".into());
        let result = client_conn.authenticate(&event, &RELAY.into());

        assert!(matches!(result, Err(Error::AuthFailure)));
    }

    #[test]
    fn test_authenticate_when_already_authenticated() {
        let mut client_conn = ClientConn::new("127.0.0.1".into());

        assert_eq!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), None);

        client_conn.generate_auth_challenge();

        assert_ne!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), None);

        let challenge = client_conn.auth_challenge().unwrap().clone();

        let event = auth_event(&challenge);
        let result = client_conn.authenticate(&event, &RELAY.into());

        assert!(matches!(result, Ok(())));
        assert_eq!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), Some(&event.pubkey));

        let event1 = auth_event(&challenge);
        let result1 = client_conn.authenticate(&event1, &RELAY.into());

        assert!(matches!(result1, Ok(())));
        assert_eq!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), Some(&event.pubkey));
        assert_ne!(client_conn.auth_pubkey(), Some(&event1.pubkey));
    }

    #[test]
    fn test_fail_to_authenticate_with_invalid_event() {
        let mut client_conn = ClientConn::new("127.0.0.1".into());

        assert_eq!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), None);

        client_conn.generate_auth_challenge();

        assert_ne!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), None);

        let challenge = client_conn.auth_challenge().unwrap();
        let mut event = auth_event(challenge);
        event.sig = event.sig.chars().rev().collect::<String>();

        let result = client_conn.authenticate(&event, &RELAY.into());

        assert!(matches!(result, Err(Error::AuthFailure)));
    }

    #[test]
    fn test_fail_to_authenticate_with_invalid_event_kind() {
        let mut client_conn = ClientConn::new("127.0.0.1".into());

        assert_eq!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), None);

        client_conn.generate_auth_challenge();

        assert_ne!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), None);

        let challenge = client_conn.auth_challenge().unwrap();
        let event = auth_event_with_kind(challenge, 9999999999999999);

        let result = client_conn.authenticate(&event, &RELAY.into());

        assert!(matches!(result, Err(Error::AuthFailure)));
    }

    #[test]
    fn test_fail_to_authenticate_with_expired_timestamp() {
        let mut client_conn = ClientConn::new("127.0.0.1".into());

        assert_eq!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), None);

        client_conn.generate_auth_challenge();

        assert_ne!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), None);

        let challenge = client_conn.auth_challenge().unwrap();
        let event = auth_event_with_created_at(challenge, unix_time() - 1200); // 20 minutes

        let result = client_conn.authenticate(&event, &RELAY.into());

        assert!(matches!(result, Err(Error::AuthFailure)));
    }

    #[test]
    fn test_fail_to_authenticate_with_future_timestamp() {
        let mut client_conn = ClientConn::new("127.0.0.1".into());

        assert_eq!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), None);

        client_conn.generate_auth_challenge();

        assert_ne!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), None);

        let challenge = client_conn.auth_challenge().unwrap();
        let event = auth_event_with_created_at(challenge, unix_time() + 1200); // 20 minutes

        let result = client_conn.authenticate(&event, &RELAY.into());

        assert!(matches!(result, Err(Error::AuthFailure)));
    }

    #[test]
    fn test_fail_to_authenticate_without_tags() {
        let mut client_conn = ClientConn::new("127.0.0.1".into());

        assert_eq!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), None);

        client_conn.generate_auth_challenge();

        assert_ne!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), None);

        let event = auth_event_without_tags();

        let result = client_conn.authenticate(&event, &RELAY.into());

        assert!(matches!(result, Err(Error::AuthFailure)));
    }

    #[test]
    fn test_fail_to_authenticate_without_challenge() {
        let mut client_conn = ClientConn::new("127.0.0.1".into());

        assert_eq!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), None);

        client_conn.generate_auth_challenge();

        assert_ne!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), None);

        let event = auth_event_without_challenge();

        let result = client_conn.authenticate(&event, &RELAY.into());

        assert!(matches!(result, Err(Error::AuthFailure)));
    }

    #[test]
    fn test_fail_to_authenticate_without_relay() {
        let mut client_conn = ClientConn::new("127.0.0.1".into());

        assert_eq!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), None);

        client_conn.generate_auth_challenge();

        assert_ne!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), None);

        let challenge = client_conn.auth_challenge().unwrap();
        let event = auth_event_without_relay(challenge);

        let result = client_conn.authenticate(&event, &RELAY.into());

        assert!(matches!(result, Err(Error::AuthFailure)));
    }

    #[test]
    fn test_fail_to_authenticate_with_invalid_challenge() {
        let mut client_conn = ClientConn::new("127.0.0.1".into());

        assert_eq!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), None);

        client_conn.generate_auth_challenge();

        assert_ne!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), None);

        let event = auth_event(&"invalid challenge".into());

        let result = client_conn.authenticate(&event, &RELAY.into());

        assert!(matches!(result, Err(Error::AuthFailure)));
    }

    #[test]
    fn test_fail_to_authenticate_with_invalid_relay() {
        let mut client_conn = ClientConn::new("127.0.0.1".into());

        assert_eq!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), None);

        client_conn.generate_auth_challenge();

        assert_ne!(client_conn.auth_challenge(), None);
        assert_eq!(client_conn.auth_pubkey(), None);

        let challenge = client_conn.auth_challenge().unwrap();
        let event = auth_event_with_relay(challenge, &"xyz".into());

        let result = client_conn.authenticate(&event, &RELAY.into());

        assert!(matches!(result, Err(Error::AuthFailure)));
    }

    fn auth_event(challenge: &String) -> Event {
        create_auth_event(Some(challenge), Some(&RELAY.into()), 22242, unix_time())
    }

    fn auth_event_with_kind(challenge: &String, kind: u64) -> Event {
        create_auth_event(Some(challenge), Some(&RELAY.into()), kind, unix_time())
    }

    fn auth_event_with_created_at(challenge: &String, created_at: u64) -> Event {
        create_auth_event(Some(challenge), Some(&RELAY.into()), 22242, created_at)
    }

    fn auth_event_without_challenge() -> Event {
        create_auth_event(None, Some(&RELAY.into()), 22242, unix_time())
    }

    fn auth_event_without_relay(challenge: &String) -> Event {
        create_auth_event(Some(challenge), None, 22242, unix_time())
    }

    fn auth_event_without_tags() -> Event {
        create_auth_event(None, None, 22242, unix_time())
    }

    fn auth_event_with_relay(challenge: &String, relay: &String) -> Event {
        create_auth_event(Some(challenge), Some(relay), 22242, unix_time())
    }

    fn create_auth_event(
        challenge: Option<&String>,
        relay: Option<&String>,
        kind: u64,
        created_at: u64,
    ) -> Event {
        let secp = Secp256k1::new();
        let key_pair = KeyPair::new(&secp, &mut rand::thread_rng());
        let public_key = XOnlyPublicKey::from_keypair(&key_pair);

        let mut tags: Vec<Vec<String>> = vec![];

        if let Some(c) = challenge {
            let tag = vec!["challenge".into(), c.into()];
            tags.push(tag);
        }

        if let Some(r) = relay {
            let tag = vec!["relay".into(), r.into()];
            tags.push(tag);
        }

        let mut event = Event {
            id: "0".to_owned(),
            pubkey: public_key.to_hex(),
            delegated_by: None,
            created_at: created_at,
            kind: kind,
            tags: tags,
            content: "".to_owned(),
            sig: "0".to_owned(),
            tagidx: None,
        };

        let c = event.to_canonical().unwrap();
        let digest: sha256::Hash = sha256::Hash::hash(c.as_bytes());

        let msg = secp256k1::Message::from_slice(digest.as_ref()).unwrap();
        let sig = secp.sign_schnorr(&msg, &key_pair);

        event.id = format!("{digest:x}");
        event.sig = sig.to_hex();

        event
    }
}
