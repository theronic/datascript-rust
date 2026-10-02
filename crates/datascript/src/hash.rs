//! ClojureScript's hashing, as `cljs.core` computes it: Murmur3 over 32-bit integers, Java's string hash under it, and
//! the collection hashes on top. DataScript's answers come out of ClojureScript's hash sets and maps, whose order of
//! iteration is the order of these hashes (see `coll`), so the port computes the same numbers.
//!
//! Everything here is 32-bit two's complement arithmetic: where `cljs.core` adds JavaScript numbers and truncates
//! later, wrapping here gives the same bits.

const M3_C1: u32 = 0xcc9e_2d51;
const M3_C2: u32 = 0x1b87_3593;

#[inline]
pub fn m3_mix_k1(k1: i32) -> i32 {
    (k1 as u32).wrapping_mul(M3_C1).rotate_left(15).wrapping_mul(M3_C2) as i32
}

#[inline]
pub fn m3_mix_h1(h1: i32, k1: i32) -> i32 {
    ((h1 ^ k1) as u32).rotate_left(13).wrapping_mul(5).wrapping_add(0xe654_6b64) as i32
}

#[inline]
pub fn m3_fmix(h1: i32, len: i32) -> i32 {
    let mut h = (h1 ^ len) as u32;
    h ^= h >> 16;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2_ae35);
    h ^= h >> 16;
    h as i32
}

/// `m3-hash-int`
#[inline]
pub fn m3_hash_int(input: i32) -> i32 {
    if input == 0 {
        0
    } else {
        m3_fmix(m3_mix_h1(0, m3_mix_k1(input)), 4)
    }
}

/// `hash-string*`: Java's `String.hashCode`, over the string's UTF-16 code units.
pub fn hash_string_raw(s: &str) -> i32 {
    let mut h: i32 = 0;
    for unit in s.encode_utf16() {
        h = h.wrapping_mul(31).wrapping_add(unit as i32);
    }
    h
}

/// `(hash "a string")`
#[inline]
pub fn hash_string(s: &str) -> i32 {
    m3_hash_int(hash_string_raw(s))
}

/// `m3-hash-unencoded-chars`, over the string's UTF-16 code units, two at a time.
pub fn m3_hash_unencoded_chars(s: &str) -> i32 {
    let mut h1: i32 = 0;
    let mut len: i32 = 0;
    let mut pending: Option<u16> = None;
    for unit in s.encode_utf16() {
        len += 1;
        match pending.take() {
            None => pending = Some(unit),
            Some(first) => {
                let k = (first as u32) | ((unit as u32) << 16);
                h1 = m3_mix_h1(h1, m3_mix_k1(k as i32));
            }
        }
    }
    if let Some(last) = pending {
        h1 ^= m3_mix_k1(last as i32);
    }
    m3_fmix(h1, len.wrapping_mul(2))
}

/// `hash-combine`, a la boost.
#[inline]
pub fn hash_combine(seed: i32, hash: i32) -> i32 {
    seed ^ hash.wrapping_add(0x9e37_79b9_u32 as i32).wrapping_add(seed.wrapping_shl(6)).wrapping_add(seed >> 2)
}

/// `hash-symbol`: of a symbol's name and namespace.
pub fn hash_symbol(ns: Option<&str>, name: &str) -> i32 {
    hash_combine(m3_hash_unencoded_chars(name), ns.map_or(0, hash_string_raw))
}

/// `hash-keyword`
pub fn hash_keyword(ns: Option<&str>, name: &str) -> i32 {
    hash_symbol(ns, name).wrapping_add(0x9e37_79b9_u32 as i32)
}

/// `mix-collection-hash`
#[inline]
pub fn mix_collection_hash(basis: i32, count: i32) -> i32 {
    m3_fmix(m3_mix_h1(0, m3_mix_k1(basis)), count)
}

/// `hash-ordered-coll`, given each element's hash in order.
pub fn hash_ordered<I: IntoIterator<Item = i32>>(hashes: I) -> i32 {
    let mut n: i32 = 0;
    let mut h: i32 = 1;
    for x in hashes {
        n = n.wrapping_add(1);
        h = h.wrapping_mul(31).wrapping_add(x);
    }
    mix_collection_hash(h, n)
}

/// `hash-unordered-coll`, given each element's hash.
pub fn hash_unordered<I: IntoIterator<Item = i32>>(hashes: I) -> i32 {
    let mut n: i32 = 0;
    let mut h: i32 = 0;
    for x in hashes {
        n = n.wrapping_add(1);
        h = h.wrapping_add(x);
    }
    mix_collection_hash(h, n)
}

/// A map entry's hash: that of the vector `[k v]`.
#[inline]
pub fn hash_map_entry(k: i32, v: i32) -> i32 {
    hash_ordered([k, v])
}

/// JavaScript's `ToInt32`.
pub fn to_int32(n: f64) -> i32 {
    if !n.is_finite() {
        return 0;
    }
    let t = n.trunc();
    // modulo 2^32, into the signed range
    let m = t.rem_euclid(4_294_967_296.0);
    (m as u32) as i32
}

const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// `Number.isSafeInteger`
#[inline]
pub fn is_safe_integer(n: f64) -> bool {
    n.is_finite() && n.fract() == 0.0 && n.abs() <= MAX_SAFE_INTEGER
}

/// `hash-double`: the double's two halves, each read big-endian from its little-endian bytes, as `cljs.core` reads
/// them through a `DataView`.
fn hash_double(f: f64) -> i32 {
    let b = f.to_le_bytes();
    let high = i32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    let low = i32::from_be_bytes([b[4], b[5], b[6], b[7]]);
    high ^ low
}

/// `(hash n)` of a number.
pub fn hash_number(n: f64) -> i32 {
    if n.is_finite() {
        if is_safe_integer(n) {
            // (js-mod (Math/floor o) 2147483647): the remainder takes the dividend's sign
            (n % 2_147_483_647.0) as i32
        } else {
            hash_double(n)
        }
    } else if n == f64::INFINITY {
        2_146_435_072
    } else if n == f64::NEG_INFINITY {
        -1_048_576
    } else {
        2_146_959_360
    }
}

/// The key ClojureScript's hash maps and sets iterate by: a `PersistentHashMap` is a trie over a hash's bits, five at
/// a time from the lowest, and walks its slots in order. So its entries come out ordered by the first five bits, then
/// the next five, and so on: by the hash with its groups of five bits reversed.
#[inline]
pub fn hamt_order(hash: i32) -> u32 {
    let h = hash as u32;
    ((h & 0x1f) << 27)
        | (((h >> 5) & 0x1f) << 22)
        | (((h >> 10) & 0x1f) << 17)
        | (((h >> 15) & 0x1f) << 12)
        | (((h >> 20) & 0x1f) << 7)
        | (((h >> 25) & 0x1f) << 2)
        | ((h >> 30) & 0x3)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ClojureScript 1.12.145's answers (conformance/oracle)
    #[test]
    fn numbers() {
        assert_eq!(hash_number(1.0), 1);
        assert_eq!(hash_number(1.5), 63551);
        assert_eq!(hash_number(-5.0), -5);
        assert_eq!(hash_number(4_294_967_296.0), 2);
        assert_eq!(hash_number(9_007_199_254_740_992.0), 16451);
        assert_eq!(hash_number(0.0), 0);
        assert_eq!(hash_number(-0.0), 0);
    }

    #[test]
    fn strings() {
        assert_eq!(hash_string("abc"), 74834163);
        assert_eq!(hash_string(""), 0);
        assert_eq!(hash_string("550e8400-e29b-41d4-a716-446655440000"), -257403564);
    }

    #[test]
    fn named() {
        assert_eq!(hash_keyword(Some("a"), "b"), 1482224565);
        assert_eq!(hash_symbol(Some("foo"), "bar"), 254379989);
    }

    #[test]
    fn collections() {
        assert_eq!(hash_ordered([1, 2, 3]), -1710654795);
        assert_eq!(hash_ordered([]), -2017569654);
        assert_eq!(hash_unordered([1, 2]), -1751407317);
        // {:a 1}
        let a = hash_keyword(None, "a");
        assert_eq!(hash_unordered([hash_map_entry(a, 1)]), -2144769406);
    }

    #[test]
    fn int32() {
        assert_eq!(to_int32(1_577_836_800_000.0), 1583802368);
        assert_eq!(to_int32(-1.5), -1);
        assert_eq!(to_int32(f64::NAN), 0);
    }
}
