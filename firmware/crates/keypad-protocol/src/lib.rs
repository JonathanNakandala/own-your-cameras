#![no_std]
#![forbid(unsafe_code)]
//! Bounded legacy AES envelopes and a staged key handoff, without hardware I/O.
//!
//! AES-ECB and CRC provide no message authentication. `Verified` means one
//! correlated idle exchange used the new key; it never authorizes entry or proves
//! complete registration, UART ownership, freshness across reconnects or safety.
//! The caller supplies bootstrap material, fresh OS-CSPRNG pending bytes and time.
//! No factory material, credentials, RNG, clock, allocation or logging lives here.

use aes::{
    Aes128,
    cipher::{Array, BlockCipherDecrypt, BlockCipherEncrypt, KeyInit},
};
/// Longest frame either side may send.
pub const MAX_WIRE_BYTES: usize = 1536;
/// Whole AES blocks that fit after the 7-byte header and 3-byte trailer.
pub const MAX_CIPHERTEXT_BYTES: usize = (MAX_WIRE_BYTES - 10) / 16 * 16;

/// CRC-16/IBM (reflected 0x8005, init 0), as the keypad bus uses it.
pub fn crc16_ibm(bytes: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &byte in bytes {
        crc ^= u16::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 == 1 { 0xa001 } else { 0 };
        }
    }
    crc
}
use zeroize::{Zeroize, Zeroizing};

const BLOCK: usize = 16;
const OVERHEAD: usize = 10;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Extent,
    Routing,
    Checksum,
    Padding,
    Opcode,
    Sequence,
    State,
    Timeout,
    ClockRollback,
    DeadlineOverflow,
    InvalidTimeout,
}

/// Secret bytes intentionally have no Debug/Clone/serialization implementation.
/// Drop wipes owned storage; moves/register spills and caller copies remain outside
/// that guarantee. AES expanded-key wiping is enabled in the pinned cipher crate.
pub struct Key([u8; 16]);
impl Key {
    /// Takes a copy and wipes the caller's supplied array immediately.
    pub fn take(bytes: &mut [u8; 16]) -> Self {
        let key = Self(*bytes);
        bytes.zeroize();
        key
    }
}
impl Drop for Key {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Opaque outgoing wire bytes. No Debug/Clone/payload formatting.
pub struct Wire {
    bytes: [u8; MAX_WIRE_BYTES],
    len: usize,
}
impl Wire {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}
impl Drop for Wire {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

/// Explicitly borrowed decrypted content; no Debug/Clone. Drop wipes the complete
/// owned buffer, including unused storage. The caller owns every copied body byte.
pub struct Message {
    bytes: [u8; MAX_CIPHERTEXT_BYTES],
    len: usize,
}
impl Message {
    pub fn sequence(&self) -> u8 {
        self.bytes[0]
    }
    pub fn body(&self) -> &[u8] {
        &self.bytes[3..3 + self.len]
    }
}
impl Drop for Message {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

/// Encode an AES-128 ECB envelope. No configurable plaintext fallback occurs here.
/// Zero padding is protocol compatibility, not PKCS#7 or authentication.
pub fn seal(key: &Key, module: u16, outer: u8, sequence: u8, body: &[u8]) -> Result<Wire, Error> {
    if body.is_empty() || body.len() > MAX_CIPHERTEXT_BYTES - 4 {
        return Err(Error::Extent);
    }
    // Legacy encoder always advances to the next block, even when aligned.
    let size = (body.len() + 3) / BLOCK * BLOCK + BLOCK;
    let mut plain = Zeroizing::new([0u8; MAX_CIPHERTEXT_BYTES]);
    plain[0] = sequence;
    plain[1..3].copy_from_slice(&(body.len() as u16).to_le_bytes());
    plain[3..3 + body.len()].copy_from_slice(body);
    let cipher = Aes128::new_from_slice(&key.0).map_err(|_| Error::Extent)?;
    let mut wire = Wire {
        bytes: [0; MAX_WIRE_BYTES],
        len: size + OVERHEAD,
    };
    wire.bytes[..2].copy_from_slice(&[0x24, 0]);
    wire.bytes[2..4].copy_from_slice(&module.to_le_bytes());
    wire.bytes[4] = outer;
    wire.bytes[5..7].copy_from_slice(&(size as u16).to_le_bytes());
    for (source, target) in plain[..size]
        .chunks_exact(BLOCK)
        .zip(wire.bytes[7..7 + size].chunks_exact_mut(BLOCK))
    {
        let mut block = Array::default();
        block.copy_from_slice(source);
        cipher.encrypt_block(&mut block);
        target.copy_from_slice(&block);
        block.as_mut_slice().zeroize();
    }
    wire.bytes[wire.len - 3] = 3;
    let crc = crc16_ibm(&wire.bytes[..wire.len - 2]);
    let end = wire.len;
    wire.bytes[end - 2..end].copy_from_slice(&crc.to_le_bytes());
    Ok(wire)
}

/// Strict response envelope validation, before interpreting decrypted body bytes.
/// `outer` must come from qualified caller policy; no request|0x80 guess is made.
pub fn open_response(key: &Key, module: u16, outer: u8, wire: &[u8]) -> Result<Message, Error> {
    if wire.len() < OVERHEAD + BLOCK || wire.len() > MAX_WIRE_BYTES {
        return Err(Error::Extent);
    }
    if wire[..2] != [0x42, 0] || wire[2..4] != module.to_le_bytes() || wire[4] != outer {
        return Err(Error::Routing);
    }
    let size = u16::from_le_bytes([wire[5], wire[6]]) as usize;
    if size == 0
        || !size.is_multiple_of(BLOCK)
        || size > MAX_CIPHERTEXT_BYTES
        || size + OVERHEAD != wire.len()
    {
        return Err(Error::Extent);
    }
    let end = wire.len();
    if wire[end - 3] != 3 {
        return Err(Error::Extent);
    }
    if crc16_ibm(&wire[..end - 2]) != u16::from_le_bytes([wire[end - 2], wire[end - 1]]) {
        return Err(Error::Checksum);
    }
    let cipher = Aes128::new_from_slice(&key.0).map_err(|_| Error::Extent)?;
    let mut message = Message {
        bytes: [0; MAX_CIPHERTEXT_BYTES],
        len: 0,
    };
    for (source, target) in wire[7..end - 3]
        .chunks_exact(BLOCK)
        .zip(message.bytes[..size].chunks_exact_mut(BLOCK))
    {
        let mut block = Array::default();
        block.copy_from_slice(source);
        cipher.decrypt_block(&mut block);
        target.copy_from_slice(&block);
        block.as_mut_slice().zeroize();
    }
    let body = u16::from_le_bytes([message.bytes[1], message.bytes[2]]) as usize;
    if body == 0 || body > size - 3 || (body + 3) / BLOCK * BLOCK + BLOCK != size {
        return Err(Error::Extent);
    }
    if message.bytes[3 + body..size].iter().any(|byte| *byte != 0) {
        return Err(Error::Padding);
    }
    message.len = body;
    Ok(message)
}

/// Dedicated key-update path ALWAYS uses AES, including configured length zero.
/// Generic unencrypted envelopes are intentionally a separate, unimplemented API.
/// This low-level function does not activate or validate either key slot.
pub fn key_update(
    key: &Key,
    module: u16,
    sequence: u8,
    configured_length: u8,
    pending: &[u8; 32],
) -> Result<Wire, Error> {
    if !matches!(configured_length, 0 | 16) {
        return Err(Error::Extent);
    }
    let mut body = Zeroizing::new([0u8; 34]);
    body[..2].copy_from_slice(&[0x77, configured_length]);
    body[2..].copy_from_slice(pending);
    seal(key, module, 0x20, sequence, body.as_ref())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyState {
    Bootstrap,
    AwaitingAck,
    NeedsVerificationPoll,
    AwaitingIdleReply,
    Verified,
    Lost,
}

struct Pending {
    bytes: [u8; 32],
    sequence: u8,
    deadline: u64,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

/// Handoff for a caller-qualified AES128 module, after registration/info/required
/// notification. This does not implement or assert that prerequisite handshake.
/// ACK outer opcode is mandatory caller policy because vendor flag1 transport
/// does not enforce it. A successful handoff is not an access grant.
pub struct Session {
    active: Option<Key>,
    pending: Option<Pending>,
    module: u16,
    ack_outer: u8,
    state: KeyState,
    last_time: u64,
    timeout: u64,
    poll: Option<(u8, u64)>,
    verification_deadline: Option<u64>,
}
impl Session {
    pub fn after_registration(
        module: u16,
        ack_outer: u8,
        now_ms: u64,
        timeout_ms: u64,
        bootstrap: &mut [u8; 16],
    ) -> Result<Self, Error> {
        if timeout_ms == 0 || timeout_ms > 5_000 {
            return Err(Error::InvalidTimeout);
        }
        Ok(Self {
            active: Some(Key::take(bootstrap)),
            pending: None,
            module,
            ack_outer,
            state: KeyState::Bootstrap,
            last_time: now_ms,
            timeout: timeout_ms,
            poll: None,
            verification_deadline: None,
        })
    }
    pub fn state(&self) -> KeyState {
        self.state
    }
    /// Loss wipes active/pending keys and all transaction continuity. Reconstruct
    /// from externally protected bootstrap input after a qualified fresh handshake.
    pub fn loss(&mut self) {
        self.pending = None;
        self.active = None;
        self.poll = None;
        self.verification_deadline = None;
        self.state = KeyState::Lost;
    }
    pub fn tick(&mut self, now_ms: u64) -> Result<(), Error> {
        if now_ms < self.last_time {
            self.loss();
            return Err(Error::ClockRollback);
        }
        self.last_time = now_ms;
        let deadline = self
            .pending
            .as_ref()
            .map(|p| p.deadline)
            .or(self.verification_deadline);
        if deadline.is_some_and(|at| now_ms >= at) {
            self.loss();
            return Err(Error::Timeout);
        }
        if self.state == KeyState::Lost {
            return Err(Error::State);
        }
        Ok(())
    }
    fn deadline(&mut self, now_ms: u64) -> Result<u64, Error> {
        match now_ms.checked_add(self.timeout) {
            Some(deadline) => Ok(deadline),
            None => {
                self.loss();
                Err(Error::DeadlineOverflow)
            }
        }
    }
    /// Takes fresh caller CSPRNG bytes only on success, wiping that input array.
    /// Retrying while busy returns State and never replaces/extends a transaction.
    pub fn begin_update(
        &mut self,
        now_ms: u64,
        sequence: u8,
        pending: &mut [u8; 32],
    ) -> Result<Wire, Error> {
        self.tick(now_ms)?;
        if !matches!(self.state, KeyState::Bootstrap | KeyState::Verified) {
            return Err(Error::State);
        }
        let deadline = self.deadline(now_ms)?;
        let wire = key_update(
            self.active.as_ref().ok_or(Error::State)?,
            self.module,
            sequence,
            16,
            pending,
        )?;
        self.pending = Some(Pending {
            bytes: *pending,
            sequence,
            deadline,
        });
        pending.zeroize();
        self.poll = None;
        self.state = KeyState::AwaitingAck;
        Ok(wire)
    }
    /// Any invalid/misordered reply loses the session. Strict sequence/outer checks
    /// deliberately exceed the legacy handler; CRC/ECB still cannot authenticate.
    pub fn accept_update_ack(&mut self, now_ms: u64, wire: &[u8]) -> Result<(), Error> {
        let result = self.check_ack(now_ms, wire);
        if result.is_err() {
            self.loss();
        }
        result
    }
    fn check_ack(&mut self, now_ms: u64, wire: &[u8]) -> Result<(), Error> {
        self.tick(now_ms)?;
        if self.state != KeyState::AwaitingAck {
            return Err(Error::State);
        }
        let message = open_response(
            self.active.as_ref().ok_or(Error::State)?,
            self.module,
            self.ack_outer,
            wire,
        )?;
        let pending = self.pending.as_ref().ok_or(Error::State)?;
        if message.sequence() != pending.sequence {
            return Err(Error::Sequence);
        }
        // Unknown trailing ACK status fields are not accepted as proven success.
        if message.body() != [0x77] {
            return Err(Error::Opcode);
        }
        let mut next = Zeroizing::new([0u8; 16]);
        next.copy_from_slice(&pending.bytes[..16]);
        self.active = Some(Key::take(&mut next));
        self.pending = None;
        self.verification_deadline = Some(self.deadline(now_ms)?);
        self.state = KeyState::NeedsVerificationPoll;
        Ok(())
    }
    pub fn verification_poll(&mut self, now_ms: u64, sequence: u8) -> Result<Wire, Error> {
        self.tick(now_ms)?;
        if self.state != KeyState::NeedsVerificationPoll {
            return Err(Error::State);
        }
        let deadline = self.verification_deadline.ok_or(Error::State)?;
        let wire = seal(
            self.active.as_ref().ok_or(Error::State)?,
            self.module,
            0x30,
            sequence,
            &[0x30, 1],
        )?;
        self.poll = Some((sequence, deadline));
        self.state = KeyState::AwaitingIdleReply;
        Ok(wire)
    }
    pub fn accept_idle_reply(&mut self, now_ms: u64, wire: &[u8]) -> Result<(), Error> {
        let result = self.check_idle(now_ms, wire);
        if result.is_err() {
            self.loss();
        }
        result
    }
    fn check_idle(&mut self, now_ms: u64, wire: &[u8]) -> Result<(), Error> {
        self.tick(now_ms)?;
        if self.state != KeyState::AwaitingIdleReply {
            return Err(Error::State);
        }
        let message = open_response(
            self.active.as_ref().ok_or(Error::State)?,
            self.module,
            0x30,
            wire,
        )?;
        if Some(message.sequence()) != self.poll.map(|p| p.0) {
            return Err(Error::Sequence);
        }
        if message.body() != [0xc2] {
            return Err(Error::Opcode);
        }
        self.poll = None;
        self.verification_deadline = None;
        self.state = KeyState::Verified;
        Ok(())
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.loss();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const BOOT: [u8; 16] = [1; 16];
    const NEXT: [u8; 32] = [2; 32];
    fn key(bytes: [u8; 16]) -> Key {
        Key::take(&mut { bytes })
    }
    fn response(key: &Key, outer: u8, seq: u8, body: &[u8]) -> Wire {
        let mut wire = seal(key, 7, outer, seq, body).unwrap();
        wire.bytes[0] = 0x42;
        repair(&mut wire);
        wire
    }
    fn repair(wire: &mut Wire) {
        let end = wire.len;
        let crc = crc16_ibm(&wire.bytes[..end - 2]);
        wire.bytes[end - 2..end].copy_from_slice(&crc.to_le_bytes());
    }
    fn session(now: u64) -> Session {
        // Outer0xa0 is only this isolated fixture's explicit policy.
        Session::after_registration(7, 0xa0, now, 100, &mut { BOOT }).unwrap()
    }
    fn ack() -> Wire {
        response(&key(BOOT), 0xa0, 255, &[0x77])
    }
    fn staged() -> Session {
        let mut s = session(10);
        s.begin_update(11, 255, &mut { NEXT }).unwrap();
        s.accept_update_ack(12, ack().as_bytes()).unwrap();
        s
    }
    #[test]
    fn takes_and_clears_supplied_key_array() {
        let mut bytes = BOOT;
        let k = Key::take(&mut bytes);
        assert_eq!(bytes, [0; 16]);
        assert_eq!(k.0, BOOT);
    }
    #[test]
    fn envelope_roundtrip_and_legacy_aligned_padding() {
        let k = key(BOOT);
        for size in [1, 2, 12, 13, 14, 34, 1516] {
            let body = [9; 1516];
            let wire = response(&k, 0x30, 255, &body[..size]);
            let m = open_response(&k, 7, 0x30, wire.as_bytes()).unwrap();
            assert_eq!(m.sequence(), 255);
            assert_eq!(m.body(), &body[..size]);
            assert_eq!(wire.len, ((size + 3) / 16 + 1) * 16 + 10);
        }
        assert_eq!(seal(&k, 7, 0x30, 0, &[9; 1517]).err(), Some(Error::Extent));
        assert_eq!(seal(&k, 7, 0x30, 0, &[]).err(), Some(Error::Extent));
    }
    #[test]
    fn update_is_encrypted_even_configured_zero() {
        for configured in [0, 16] {
            let k = key(BOOT);
            let mut w = key_update(&k, 7, 255, configured, &NEXT).unwrap();
            assert_eq!(w.len, 58);
            w.bytes[0] = 0x42;
            repair(&mut w);
            let m = open_response(&k, 7, 0x20, w.as_bytes()).unwrap();
            assert_eq!(m.body()[..2], [0x77, configured]);
            assert_eq!(m.body()[2..], NEXT);
        }
        assert_eq!(
            key_update(&key(BOOT), 7, 0, 24, &NEXT).err(),
            Some(Error::Extent)
        );
    }
    #[test]
    fn pending_commits_only_ack_then_correlated_new_key_idle() {
        let mut s = session(10);
        let mut next = NEXT;
        s.begin_update(11, 255, &mut next).unwrap();
        assert_eq!(next, [0; 32]);
        assert_eq!(s.active.as_ref().unwrap().0, BOOT);
        assert_eq!(s.state(), KeyState::AwaitingAck);
        s.accept_update_ack(12, ack().as_bytes()).unwrap();
        assert_eq!(s.active.as_ref().unwrap().0, [2; 16]);
        assert!(s.pending.is_none());
        assert_eq!(s.state(), KeyState::NeedsVerificationPoll);
        let poll = s.verification_poll(13, 0).unwrap();
        assert_eq!(poll.len, 26);
        assert_eq!(s.state(), KeyState::AwaitingIdleReply);
        let idle = response(&key([2; 16]), 0x30, 0, &[0xc2]);
        s.accept_idle_reply(14, idle.as_bytes()).unwrap();
        assert_eq!(s.state(), KeyState::Verified);
    }
    #[test]
    fn retry_cannot_replace_pending_or_extend_deadline() {
        let mut s = session(10);
        s.begin_update(11, 255, &mut { NEXT }).unwrap();
        let mut retry = [3; 32];
        assert_eq!(
            s.begin_update(100, 255, &mut retry).err(),
            Some(Error::State)
        );
        assert_eq!(retry, [3; 32]);
        assert_eq!(s.pending.as_ref().unwrap().deadline, 111);
        assert_eq!(s.tick(111), Err(Error::Timeout));
        assert!(s.active.is_none());
        assert!(s.pending.is_none());
    }
    #[test]
    fn exact_deadline_and_rollback_erase_session() {
        let mut s = session(10);
        s.begin_update(11, 255, &mut { NEXT }).unwrap();
        assert_eq!(
            s.accept_update_ack(111, ack().as_bytes()),
            Err(Error::Timeout)
        );
        assert_eq!(s.state(), KeyState::Lost);
        let mut s = staged();
        assert_eq!(s.tick(11), Err(Error::ClockRollback));
        assert!(s.active.is_none());
        assert_eq!(s.tick(13), Err(Error::State));
    }
    #[test]
    fn overflow_and_invalid_timeout_do_not_create_unbounded_transaction() {
        assert_eq!(
            Session::after_registration(7, 0xa0, 0, 0, &mut { BOOT }).err(),
            Some(Error::InvalidTimeout)
        );
        assert_eq!(
            Session::after_registration(7, 0xa0, 0, 5001, &mut { BOOT }).err(),
            Some(Error::InvalidTimeout)
        );
        let mut s = session(u64::MAX - 1);
        assert_eq!(
            s.begin_update(u64::MAX, 1, &mut { NEXT }).err(),
            Some(Error::DeadlineOverflow)
        );
        assert_eq!(s.state(), KeyState::Lost);
        assert!(s.active.is_none());
    }
    #[test]
    fn ack_sequence_opcode_and_body_are_required() {
        for (outer, seq, body) in [
            (0x30, 255, &[0x77][..]),
            (0xa0, 0, &[0x77][..]),
            (0xa0, 255, &[0x76][..]),
            (0xa0, 255, &[0x77, 1][..]),
        ] {
            let mut s = session(10);
            s.begin_update(11, 255, &mut { NEXT }).unwrap();
            let w = response(&key(BOOT), outer, seq, body);
            assert!(s.accept_update_ack(12, w.as_bytes()).is_err());
            assert!(s.active.is_none());
            assert!(s.pending.is_none());
        }
    }
    #[test]
    fn malformed_wire_is_rejected_before_plaintext_use() {
        for offset in [0, 1, 2, 3, 4, 5, 6, 23, 24, 25] {
            let k = key(BOOT);
            let mut w = response(&k, 0x30, 1, &[0xc2]);
            w.bytes[offset] ^= 1;
            assert!(open_response(&k, 7, 0x30, w.as_bytes()).is_err());
        }
        let k = key(BOOT);
        let w = response(&k, 0x30, 1, &[0xc2]);
        for cut in 0..w.len {
            assert!(open_response(&k, 7, 0x30, &w.as_bytes()[..cut]).is_err());
        }
    }
    #[test]
    fn nonzero_padding_and_invalid_inner_length_refuse() {
        let k = key(BOOT);
        for bytes in [[1u8; 16], [0u8; 16]] {
            let mut w = response(&k, 0x30, 1, &[0xc2]);
            let cipher = Aes128::new_from_slice(&BOOT).unwrap();
            let mut block = Array::from(bytes);
            cipher.encrypt_block(&mut block);
            w.bytes[7..23].copy_from_slice(&block);
            repair(&mut w);
            assert!(open_response(&k, 7, 0x30, w.as_bytes()).is_err());
        }
        let mut w = response(&k, 0x30, 1, &[0xc2]);
        let cipher = Aes128::new_from_slice(&BOOT).unwrap();
        let mut plain = [0; 16];
        plain[..5].copy_from_slice(&[1, 1, 0, 0xc2, 1]);
        let mut block = Array::from(plain);
        cipher.encrypt_block(&mut block);
        w.bytes[7..23].copy_from_slice(&block);
        repair(&mut w);
        assert_eq!(
            open_response(&k, 7, 0x30, w.as_bytes()).err(),
            Some(Error::Padding)
        );
    }
    #[test]
    fn idle_failure_or_old_key_loses_handoff() {
        for (seq, body, bytes) in [
            (1, &[0xc2][..], [2; 16]),
            (0, &[0xc4][..], [2; 16]),
            (0, &[0xc2][..], BOOT),
        ] {
            let mut s = staged();
            s.verification_poll(13, 0).unwrap();
            let w = response(&key(bytes), 0x30, seq, body);
            assert!(s.accept_idle_reply(14, w.as_bytes()).is_err());
            assert_eq!(s.state(), KeyState::Lost);
        }
    }
    #[test]
    fn loss_during_any_stage_erases_keys_and_continuity() {
        let mut s = session(10);
        s.loss();
        assert!(s.active.is_none());
        assert!(s.pending.is_none());
        let mut s = staged();
        s.verification_poll(13, 0).unwrap();
        s.loss();
        assert!(s.active.is_none());
        assert!(s.poll.is_none());
        assert_eq!(s.state(), KeyState::Lost);
    }
    #[test]
    fn duplicate_ack_cannot_reactivate_or_skip_verification() {
        let mut s = staged();
        assert!(s.accept_update_ack(13, ack().as_bytes()).is_err());
        assert_eq!(s.state(), KeyState::Lost);
    }
    #[test]
    fn idle_reply_exact_deadline_refuses() {
        let mut s = staged();
        s.verification_poll(13, 0).unwrap();
        let w = response(&key([2; 16]), 0x30, 0, &[0xc2]);
        assert_eq!(s.accept_idle_reply(112, w.as_bytes()), Err(Error::Timeout));
        assert!(s.active.is_none());
    }
    #[test]
    fn verification_window_includes_wait_before_poll_and_cannot_extend() {
        let mut s = staged();
        assert_eq!(s.tick(112), Err(Error::Timeout));
        assert!(s.active.is_none());
        let mut s = staged();
        s.verification_poll(111, 0).unwrap();
        let reply = response(&key([2; 16]), 0x30, 0, &[0xc2]);
        assert_eq!(
            s.accept_idle_reply(112, reply.as_bytes()),
            Err(Error::Timeout)
        );
        assert_eq!(s.state(), KeyState::Lost);
    }
    #[test]
    fn extra_zero_plaintext_blocks_cannot_verify_idle_or_ack() {
        for (bytes, outer, body) in [(BOOT, 0xa0, 0x77), ([2; 16], 0x30, 0xc2)] {
            let mut wire = response(&key(bytes), outer, 0, &[body]);
            let cipher = Aes128::new_from_slice(&bytes).unwrap();
            let mut extra = Array::from([0u8; 16]);
            cipher.encrypt_block(&mut extra);
            wire.bytes[23..39].copy_from_slice(&extra);
            wire.len = 42;
            wire.bytes[5..7].copy_from_slice(&32u16.to_le_bytes());
            wire.bytes[39] = 3;
            repair(&mut wire);
            assert_eq!(
                open_response(&key(bytes), 7, outer, wire.as_bytes()).err(),
                Some(Error::Extent)
            );
            if outer == 0x30 {
                let mut s = staged();
                s.verification_poll(13, 0).unwrap();
                assert_eq!(s.accept_idle_reply(14, wire.as_bytes()), Err(Error::Extent));
                assert_eq!(s.state(), KeyState::Lost);
            }
        }
    }
}
