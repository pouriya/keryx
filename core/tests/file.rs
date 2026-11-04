use keryx_core::file::{
    BUNDLE_FILES, CHUNK, FileError, TAG, blurhash_of, blurhash_pixels, chunk_plain, chunks_for,
    new_key, open_chunk, seal_chunk, sealed_size,
};

#[test]
fn a_file_is_cut_into_whole_chunks_and_an_empty_one_is_one_tag() {
    assert_eq!(chunks_for(0), 1);
    assert_eq!(chunks_for(1), 1);
    assert_eq!(chunks_for(CHUNK), 1);
    assert_eq!(chunks_for(CHUNK + 1), 2);
    assert_eq!(sealed_size(0), TAG);
    assert_eq!(sealed_size(CHUNK + 1), CHUNK + 1 + 2 * TAG);
    assert_eq!(chunk_plain(CHUNK + 5, 0), CHUNK);
    assert_eq!(chunk_plain(CHUNK + 5, 1), 5);
    assert_eq!(chunk_plain(0, 0), 0);
    assert_eq!(BUNDLE_FILES, 20);
}

#[test]
fn a_chunk_opens_only_as_itself_in_its_place() {
    let key = new_key().unwrap();
    let other = new_key().unwrap();
    assert_ne!(key.as_ref(), other.as_ref());
    let sealed = seal_chunk(key.as_ref(), 3, false, b"the middle").unwrap();
    assert_eq!(sealed.len() as u64, 10 + TAG);
    assert_eq!(
        open_chunk(key.as_ref(), 3, false, &sealed)
            .unwrap()
            .as_slice(),
        b"the middle"
    );
    // Moved, called the last, under another key, or changed: none of them open.
    assert!(matches!(
        open_chunk(key.as_ref(), 4, false, &sealed),
        Err(FileError::Open { n: 4 })
    ));
    assert!(open_chunk(key.as_ref(), 3, true, &sealed).is_err());
    assert!(open_chunk(other.as_ref(), 3, false, &sealed).is_err());
    let mut changed = sealed.clone();
    changed[0] ^= 1;
    assert!(open_chunk(key.as_ref(), 3, false, &changed).is_err());
    // A cut file: its new last chunk was not sealed as the last.
    let last = seal_chunk(key.as_ref(), 0, true, b"").unwrap();
    assert_eq!(last.len() as u64, TAG);
    assert!(open_chunk(key.as_ref(), 0, true, &last).unwrap().is_empty());
    assert!(matches!(
        seal_chunk(&[0; 5], 0, true, b"x"),
        Err(FileError::BadKey)
    ));
    assert!(matches!(
        open_chunk(&[0; 5], 0, true, &last),
        Err(FileError::BadKey)
    ));
}

#[test]
fn a_blurhash_is_the_standard_text_and_draws_back_as_pixels() {
    let rgba: Vec<u8> = (0..4 * 4 * 4).map(|at| (at * 7) as u8).collect();
    let hash = blurhash_of(4, 4, &rgba).unwrap();
    // 4 by 3 components: one size flag, one maximum, four for the average, two per other.
    assert_eq!(hash.len(), 6 + 2 * 11);
    assert_eq!(blurhash_pixels(&hash, 8, 6).unwrap().len(), 8 * 6 * 4);
    // Pixels that are not the size they claim are refused, not read past their end.
    assert!(matches!(
        blurhash_of(4, 4, &rgba[..3]),
        Err(FileError::BadPixels)
    ));
    assert!(matches!(blurhash_of(0, 4, &[]), Err(FileError::BadPixels)));
    assert!(matches!(
        blurhash_pixels("no", 8, 6),
        Err(FileError::Blurhash(_))
    ));
}
