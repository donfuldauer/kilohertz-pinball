# S4 zip corpus listing

The corpus (33 zips, 77 MB, includes zip bombs) is NOT archived. Regenerate with:

    python3 -I harness/make_zip_corpus.py <out_dir> <production.pinbundle>

Intended outcome is the classification under the web limits (S4 RESULTS.md, zip/VFS hardening); identical on native and wasm.

| file | size (B) | sha256 | intended outcome |
|---|---|---|---|
| 00_production_bundle.zip | 1976314 | 50c0d88f38df80d65e1481195756b6ef08e5b4a7b1d32996f5265e2d6511cc5c | accepted (production bundle, 56 entries) |
| 01_small_ok.zip | 233 | 556d253bb2a2bb43e61499999307e8a1f15f572dc0ccaf105cd04593a8e7d4d2 | accepted |
| 10_bomb_ratio_single_50MiB.zip | 51086 | de29e59b9e37f679608416176382fe3dfeac5a352247f62604562b24ed25d7ed | refused: file too large (web) / ratio (default) |
| 11_bomb_total_600x1MiB.zip | 676002 | d87e9da7192acd3afe70c4175a42f33e4e853f3218807e3a41abd23889f9b798 | refused: bundle too large (web) / archive ratio (default) |
| 12_bomb_archive_ratio_100x1MiB.zip | 112502 | ed934418edb56fcebce58428f9afe1bdf1b94c2357b3283a6c7709d039a24ac4 | refused: archive ratio |
| 13_bomb_70MiB_ratio_ok_web_cap.zip | 73406762 | a610939090f26f4363f4c9cabd7b23bcc4b973d4489f86509398b3acb82f596a | refused under web limits (bundle too large); accepted under default |
| 14_bomb_declared_size_lie.zip | 4192 | aac2da46894a0a0c0fa0029a49d77799b388aec0548da4266399488976a59d9f | opens; read refused: decompressed size differs from declared |
| 20_traversal_dotdot.zip | 237 | 6bbe58e3206bd4d77857e08ba72cfc47acbe187d96c41bd4467dee26d7d98baf | refused: path escapes bundle |
| 21_traversal_nested_dotdot.zip | 247 | f61517c08794f0c313bf8ca256e4cba30358f7cf2b3c88e79ad7812d7fdcbf6a | refused: path escapes bundle |
| 22_absolute_path.zip | 237 | db22bf04ed00ae6b329b2d13077c89e4e8203c83542871204719925227bd8159 | refused: absolute path |
| 23_backslash_traversal.zip | 237 | e1d97c6914ba2cc1a0132557196654413221a3ff97bcabfb78bbcd634cc4eabb | refused: path escapes bundle |
| 24_drive_letter.zip | 237 | 3c2ae03e69a84ecb4a7e61cd59fcbf64e8316ab14a3bf91bdfc0433cdab1c509 | refused: absolute path |
| 25_nul_in_name.zip | 229 | 6610511007a1484189fad18ddb3268b3bb91a544fd3e07b545da825ac40b112f | refused: not valid UTF-8 |
| 26_non_utf8_name.zip | 229 | 69725a74bcf2400fcc09f4ee0825370841580ad46fbe7a5a7aac2688a4d2cade | refused: not valid UTF-8 |
| 30_too_many_entries_10001.zip | 877892 | f52b43d524d226137d9b1106c21de64d59ed0801d990e9fcffa6144b76bc598c | refused: too many entries |
| 31_duplicate_names.zip | 318 | ee301b0f618fb2cf99b89f58281c25bea5d4144f907195dad3d43a3d8d0bbbf9 | refused: duplicate entry |
| 32_case_collision.zip | 318 | 5970ed4de4ce19e9ad3f40834bfb67130edced6958df208d979dcb03a9a2d749 | accepted (case-sensitive paths) |
| 33_symlink_entry.zip | 233 | fba3db7a2b992004ba6e260adbf225a4490a882ebf39497809a710688d29b981 | refused: symlink entry |
| 34_nested_zip_bomb_inside.zip | 20731 | 5bf23afb06a87bff81299657ea4abbfcd2588090038b803f7cb36ec5266da003 | accepted as opaque nested file, never extracted |
| 35_encrypted_flag.zip | 238 | fe821195a36b3521ced6a1bfde83d72c526de2db65b6173e41a8e83509cf919a | refused: encrypted |
| 36_bad_crc.zip | 236 | b7fce95d9c8a10667c05bd09e62df61fdbb7e2f57c569b64765b835d5e0a942b | opens; every read refused: CRC mismatch |
| 37_stored_method_ok.zip | 226 | 025ff1f1812d07e18b4ad2c24d7155897233ff372c7f3e721dcb1ec8063940df | accepted (stored method) |
| 38_unsupported_method_bzip2.zip | 230 | b2092f097c2959b70657a76a91d4a79b06d2162d57850e115c44a10e6ac1d844 | refused: unsupported method |
| 40_zip64_forced_ok.zip | 279 | ee503725c2b1b1bc62d8575a27a39711f80e4b86eb6ac26cb47d5a9e9cc93a0b | accepted (forced ZIP64) |
| 41_zip64_bad_locator.zip | 156 | 7e6ac33809c191044efef0e1cf079c28c1e45fd540949444b48eeb7c321a6df3 | refused: ZIP64 offset outside archive |
| 42_truncated_central_directory.zip | 1976274 | eec68c9d510534d2a13785e172292c1404725f3e282529c669bb731ea2f77624 | refused: end-of-central-directory checks |
| 43_truncated_half.zip | 988157 | fe4303eeb46016ceee71a365e96a3522a99fce88e378ba8dd17d1cdae39b98d2 | refused: end-of-central-directory checks |
| 44_prepended_junk.zip | 1138 | 7e8ced7ad0e95685bebdeb456e1cc6d5d4a14a4656d09f3363b3e8bd3b5d3c65 | refused: end-of-central-directory checks |
| 45_trailing_junk.zip | 176 | c25fdf345d4a95997e0df86b1c009c230cbf14fd613d724fc2a393b10647b3bd | refused: end-of-central-directory checks |
| 46_comment_length_lie.zip | 136 | 493a82514c1e2f13c1452ddd54906f17507eb4386b232c4cb8cd775e91dd9c76 | refused: end-of-central-directory checks |
| 47_empty_file.zip | 0 | e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 | refused: empty file |
| 48_not_a_zip.zip | 290 | 3de6946f7053f0e9e0fdf94c6097f292f5b560915db2bfcba6f0b80a6d14e5c8 | refused: not a zip |
| 49_two_eocd_records.zip | 158 | 7005b68afba3401abf25274cc125c8a2648869ae9e09a4a3e3403ffb9a5aa627 | refused: end-of-central-directory checks |
