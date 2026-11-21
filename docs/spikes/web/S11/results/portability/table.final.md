| shell | check | result | evidence |
|---|---|---|---|
| all         | ShellCheck --shell=sh -x on scripts/web/*.sh: no findings          | PASS | version: 0.9.0 |
| bash-3.2    | bash 3.2.57 (= macOS /bin/sh)                                      | SKIP | not built here (not permitted in this session); web-build-macos CI runs /bin/sh |
| dash        | platform Linux/x86_64 (incl. WSL2) -> linux-x86_64                 | PASS |  |
| dash        | platform Darwin/arm64 -> darwin-arm64                              | PASS |  |
| dash        | platform Darwin/x86_64, proc_translated=0 (Intel) -> darwin-x86_64 | PASS |  |
| dash        | platform Darwin/x86_64, no proc_translated key -> darwin-x86_64    | PASS |  |
| dash        | platform Darwin/x86_64, proc_translated=1 (Rosetta 2) -> darwin-arm64 | PASS | note: x86_64 shell under Rosetta 2 on Apple Silicon: using the darwin-arm64 tools  |
| dash        | platform Linux/aarch64 -> refused, exit 3                          | PASS | error: unsupported build host Linux/aarch64; supported: Linux x86_64 (incl. WSL2), macOS arm64 (Appl |
| dash        | platform MINGW64 (Git Bash on Windows) -> refused, exit 3          | PASS | error: Windows is not a supported build host for the web version; use WSL2 (Linux x86_64)  |
| dash        | platform FreeBSD/amd64 -> refused, exit 3                          | PASS | error: unsupported build host FreeBSD/amd64; supported: Linux x86_64 (incl. WSL2), macOS arm64 (Appl |
| dash        | khz_sha256 via node (file name with a space)                       | PASS | 1548f55de631fd1f |
| dash        | khz_sha256 via shasum (file name with a space)                     | PASS | 1548f55de631fd1f |
| dash        | khz_sha256 via sha256sum (file name with a space)                  | PASS | 1548f55de631fd1f |
| dash        | khz_sha256 auto without node -> shasum                             | PASS |  |
| dash        | khz_sha256 auto without node -> sha256sum                          | PASS |  |
| dash        | khz_sha256 with no tool: clear error                               | PASS | error: no sha256 tool: need node, shasum or sha256sum |
| dash        | khz_size / khz_ncpu without stat -c / nproc                        | PASS | 1000003 4 |
| dash        | fetch_tools --print selects the linux-x86_64 pins                  | PASS | wasi-sdk, binaryen 195ddc94f9bc, node + 2 cargo tools |
| dash        | fetch_tools --print selects the darwin-arm64 pins                  | PASS | wasi-sdk, binaryen 98aad827847a, node + 2 cargo tools |
| dash        | fetch_tools --print selects the darwin-x86_64 pins                 | PASS | wasi-sdk, binaryen 40c3de90bb37, node + 2 cargo tools |
| dash        | fetch, sha256 verify, extract; tools dir with a space              | PASS | tools.env written |
| dash        | macOS: com.apple.quarantine removed after verification             | PASS | xattr -dr ... called per tool |
| dash        | second run: cache hit, nothing fetched                             | PASS | 2 x already verified |
| dash        | wrong pin: exit 1, archive deleted, not extracted (xattr failing ok) | PASS | checksum mismatch for toolB-2.tar.gz |
| dash        | real pins linux-x86_64: wasi-sdk, binaryen, node verified, extracted | PASS | sha256 OK x3; smoke OK x3 |
| dash        | real pins darwin-arm64: wasi-sdk, binaryen, node verified, extracted | PASS | sha256 OK x3; smoke OK x0 |
| dash        | real pins darwin-x86_64: wasi-sdk, binaryen, node verified, extracted | PASS | sha256 OK x3; smoke OK x0 |
| dash        | fetched Linux trees == S2 wasi-sdk / S9 binaryen (same pins)       | PASS | binaryen: +binaryen-unittests (S9 deleted it) |
| dash        | wasm_env: host CFLAGS/CPATH/RUSTFLAGS dropped, remap intact with spaces | PASS | 4 remap entries |
| dash        | build_web.sh --dry-run on Linux: all checks pass                   | PASS | plan + env printed |
| dash        | build_web.sh refuses another platform's tools                      | PASS | tools in /tmp/claude-0/-home-user-kilohertz-pinball/13c09974-f5b8-57fb |
| dash        | macOS without CLT: refused with xcode-select --install hint        | PASS |  |
| dash        | macOS (shimmed) with CLT: dry run passes                           | PASS |  |
| dash        | size_report.sh on S9's module = S9 §8 numbers                     | PASS | 10.62 / 3.93 / 2.79 MiB |
| dash        | size_report.sh budget exceeded -> exit 1                           | PASS |  |
| dash        | check_imports.sh: S9 module PASS (623 glue imports)                | PASS |  |
| dash        | check_imports.sh: env.saveSetjmp import -> FAIL                    | PASS |  |
| dash        | no GNU-only tool flag used (trap log empty)                        | PASS |  |
| bash-posix  | platform Linux/x86_64 (incl. WSL2) -> linux-x86_64                 | PASS |  |
| bash-posix  | platform Darwin/arm64 -> darwin-arm64                              | PASS |  |
| bash-posix  | platform Darwin/x86_64, proc_translated=0 (Intel) -> darwin-x86_64 | PASS |  |
| bash-posix  | platform Darwin/x86_64, no proc_translated key -> darwin-x86_64    | PASS |  |
| bash-posix  | platform Darwin/x86_64, proc_translated=1 (Rosetta 2) -> darwin-arm64 | PASS | note: x86_64 shell under Rosetta 2 on Apple Silicon: using the darwin-arm64 tools  |
| bash-posix  | platform Linux/aarch64 -> refused, exit 3                          | PASS | error: unsupported build host Linux/aarch64; supported: Linux x86_64 (incl. WSL2), macOS arm64 (Appl |
| bash-posix  | platform MINGW64 (Git Bash on Windows) -> refused, exit 3          | PASS | error: Windows is not a supported build host for the web version; use WSL2 (Linux x86_64)  |
| bash-posix  | platform FreeBSD/amd64 -> refused, exit 3                          | PASS | error: unsupported build host FreeBSD/amd64; supported: Linux x86_64 (incl. WSL2), macOS arm64 (Appl |
| bash-posix  | khz_sha256 via node (file name with a space)                       | PASS | 1548f55de631fd1f |
| bash-posix  | khz_sha256 via shasum (file name with a space)                     | PASS | 1548f55de631fd1f |
| bash-posix  | khz_sha256 via sha256sum (file name with a space)                  | PASS | 1548f55de631fd1f |
| bash-posix  | khz_sha256 auto without node -> shasum                             | PASS |  |
| bash-posix  | khz_sha256 auto without node -> sha256sum                          | PASS |  |
| bash-posix  | khz_sha256 with no tool: clear error                               | PASS | error: no sha256 tool: need node, shasum or sha256sum |
| bash-posix  | khz_size / khz_ncpu without stat -c / nproc                        | PASS | 1000003 4 |
| bash-posix  | fetch_tools --print selects the linux-x86_64 pins                  | PASS | wasi-sdk, binaryen 195ddc94f9bc, node + 2 cargo tools |
| bash-posix  | fetch_tools --print selects the darwin-arm64 pins                  | PASS | wasi-sdk, binaryen 98aad827847a, node + 2 cargo tools |
| bash-posix  | fetch_tools --print selects the darwin-x86_64 pins                 | PASS | wasi-sdk, binaryen 40c3de90bb37, node + 2 cargo tools |
| bash-posix  | fetch, sha256 verify, extract; tools dir with a space              | PASS | tools.env written |
| bash-posix  | macOS: com.apple.quarantine removed after verification             | PASS | xattr -dr ... called per tool |
| bash-posix  | second run: cache hit, nothing fetched                             | PASS | 2 x already verified |
| bash-posix  | wrong pin: exit 1, archive deleted, not extracted (xattr failing ok) | PASS | checksum mismatch for toolB-2.tar.gz |
| bash-posix  | wasm_env: host CFLAGS/CPATH/RUSTFLAGS dropped, remap intact with spaces | PASS | 4 remap entries |
| bash-posix  | build_web.sh --dry-run on Linux: all checks pass                   | PASS | plan + env printed |
| bash-posix  | build_web.sh refuses another platform's tools                      | PASS | tools in /tmp/claude-0/-home-user-kilohertz-pinball/13c09974-f5b8-57fb |
| bash-posix  | macOS without CLT: refused with xcode-select --install hint        | PASS |  |
| bash-posix  | macOS (shimmed) with CLT: dry run passes                           | PASS |  |
| bash-posix  | size_report.sh on S9's module = S9 §8 numbers                     | PASS | 10.62 / 3.93 / 2.79 MiB |
| bash-posix  | size_report.sh budget exceeded -> exit 1                           | PASS |  |
| bash-posix  | check_imports.sh: S9 module PASS (623 glue imports)                | PASS |  |
| bash-posix  | check_imports.sh: env.saveSetjmp import -> FAIL                    | PASS |  |
| bash-posix  | no GNU-only tool flag used (trap log empty)                        | PASS |  |
| busybox-sh  | platform Linux/x86_64 (incl. WSL2) -> linux-x86_64                 | PASS |  |
| busybox-sh  | platform Darwin/arm64 -> darwin-arm64                              | PASS |  |
| busybox-sh  | platform Darwin/x86_64, proc_translated=0 (Intel) -> darwin-x86_64 | PASS |  |
| busybox-sh  | platform Darwin/x86_64, no proc_translated key -> darwin-x86_64    | PASS |  |
| busybox-sh  | platform Darwin/x86_64, proc_translated=1 (Rosetta 2) -> darwin-arm64 | PASS | note: x86_64 shell under Rosetta 2 on Apple Silicon: using the darwin-arm64 tools  |
| busybox-sh  | platform Linux/aarch64 -> refused, exit 3                          | PASS | error: unsupported build host Linux/aarch64; supported: Linux x86_64 (incl. WSL2), macOS arm64 (Appl |
| busybox-sh  | platform MINGW64 (Git Bash on Windows) -> refused, exit 3          | PASS | error: Windows is not a supported build host for the web version; use WSL2 (Linux x86_64)  |
| busybox-sh  | platform FreeBSD/amd64 -> refused, exit 3                          | PASS | error: unsupported build host FreeBSD/amd64; supported: Linux x86_64 (incl. WSL2), macOS arm64 (Appl |
| busybox-sh  | khz_sha256 via node (file name with a space)                       | PASS | 1548f55de631fd1f |
| busybox-sh  | khz_sha256 via shasum (file name with a space)                     | PASS | 1548f55de631fd1f |
| busybox-sh  | khz_sha256 via sha256sum (file name with a space)                  | PASS | 1548f55de631fd1f |
| busybox-sh  | khz_sha256 auto without node -> shasum                             | PASS |  |
| busybox-sh  | khz_sha256 auto without node -> sha256sum                          | PASS |  |
| busybox-sh  | khz_sha256 with no tool: clear error                               | PASS | error: no sha256 tool: need node, shasum or sha256sum |
| busybox-sh  | khz_size / khz_ncpu without stat -c / nproc                        | PASS | 1000003 4 |
| busybox-sh  | fetch_tools --print selects the linux-x86_64 pins                  | PASS | wasi-sdk, binaryen 195ddc94f9bc, node + 2 cargo tools |
| busybox-sh  | fetch_tools --print selects the darwin-arm64 pins                  | PASS | wasi-sdk, binaryen 98aad827847a, node + 2 cargo tools |
| busybox-sh  | fetch_tools --print selects the darwin-x86_64 pins                 | PASS | wasi-sdk, binaryen 40c3de90bb37, node + 2 cargo tools |
| busybox-sh  | fetch, sha256 verify, extract; tools dir with a space              | PASS | tools.env written |
| busybox-sh  | macOS: com.apple.quarantine removed after verification             | PASS | xattr -dr ... called per tool |
| busybox-sh  | second run: cache hit, nothing fetched                             | PASS | 2 x already verified |
| busybox-sh  | wrong pin: exit 1, archive deleted, not extracted (xattr failing ok) | PASS | checksum mismatch for toolB-2.tar.gz |
| busybox-sh  | real pins linux-x86_64: wasi-sdk, binaryen, node verified, extracted | PASS | sha256 OK x3; smoke OK x3 |
| busybox-sh  | real pins darwin-arm64: wasi-sdk, binaryen, node verified, extracted | PASS | sha256 OK x3; smoke OK x0 |
| busybox-sh  | real pins darwin-x86_64: wasi-sdk, binaryen, node verified, extracted | PASS | sha256 OK x3; smoke OK x0 |
| busybox-sh  | fetched Linux trees == S2 wasi-sdk / S9 binaryen (same pins)       | PASS | binaryen: +binaryen-unittests (S9 deleted it) |
| busybox-sh  | wasm_env: host CFLAGS/CPATH/RUSTFLAGS dropped, remap intact with spaces | PASS | 4 remap entries |
| busybox-sh  | build_web.sh --dry-run on Linux: all checks pass                   | PASS | plan + env printed |
| busybox-sh  | build_web.sh refuses another platform's tools                      | PASS | tools in /tmp/claude-0/-home-user-kilohertz-pinball/13c09974-f5b8-57fb |
| busybox-sh  | macOS without CLT: refused with xcode-select --install hint        | PASS |  |
| busybox-sh  | macOS (shimmed) with CLT: dry run passes                           | PASS |  |
| busybox-sh  | size_report.sh on S9's module = S9 §8 numbers                     | PASS | 10.62 / 3.93 / 2.79 MiB |
| busybox-sh  | size_report.sh budget exceeded -> exit 1                           | PASS |  |
| busybox-sh  | check_imports.sh: S9 module PASS (623 glue imports)                | PASS |  |
| busybox-sh  | check_imports.sh: env.saveSetjmp import -> FAIL                    | PASS |  |
| busybox-sh  | no GNU-only tool flag used (trap log empty)                        | PASS |  |
