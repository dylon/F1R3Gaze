# F1R3Gaze distribution design and build guide

## Components and compatibility

F1R3Gaze contains the RSpace it needs to execute f1r3lang inside the application. The `f1r3c` command is shipped alongside the browser. F1R3Node is a separate, optional service: a customer does not need it merely to launch Gaze. Embers is not installed by any Gaze package. The release catalog currently lists only Gaze. A future organization installer may present several independently upgradable systems after each has an installer and the catalog records the exact combinations tested together.

Linux package managers install declared system libraries and can express versioned package dependencies. macOS DMGs are app delivery images, while component PKGs can be chained by a distribution PKG. Windows MSIs are independent components; a WiX Burn bootstrapper can select and chain them. The templates in `packaging/macos/organization-distribution.xml.in` and `packaging/windows/organization.wxs.in` deliberately expose only Gaze. The selector validator rejects an unknown system or an untested pair.

![Artifact and promotion flow](packaging-flow.svg)

## Artifact matrix

| Target | Artifact | Builder | Current state |
| --- | --- | --- | --- |
| Debian 12+/Ubuntu 22.04+ x86_64, arm64 | `.deb` | Ubuntu 22.04 runner; x86_64 build available locally | Both architectures passed lower-version upgrade, launch and removal on all ten distro targets |
| Arch x86_64; Arch Linux ARM aarch64 | `.pkg.tar.zst` | `makepkg` on matching architecture | Both architectures passed build, lower-version upgrade, launch and removal in fork CI |
| Fedora 43/44, Enterprise Linux 9/10, openSUSE Leap 16 | `.rpm` | Matching distro and architecture container | All ten distro/architecture jobs passed build, lower-version upgrade, launch and removal in fork CI |
| Linux x86_64, aarch64 | `.tar.gz` and `.AppImage` | Matching Linux runner | Both architectures built and launched in fork CI |
| macOS Intel and Apple Silicon | universal `.dmg` and component `.pkg` | macOS 14 ARM runner; macOS 15 Intel installer runner | Unsigned build, DMG mount, PKG upgrade and installed URL delivery passed on both CPU families; signing and notarization await credentials |
| Windows 11 x64 | `.msi` and portable `.zip` | Windows Server 2022 runner; local cross-build for ZIP | Unsigned build, MSI major upgrade, installed running-app URL delivery, removal and ZIP launch passed; Windows 11 device pending |
| Linux sandbox channels | `.flatpak` and `.snap` | Matching Linux runners | Both architectures pass install, page/network/file access, URL desktop metadata, visible X11 window startup with Vulkan selected, and removal in fork CI |

The branch smoke workflows build the actual distribution payloads. Linux jobs build DEB, tarball and AppImage on native x86_64 and ARM64 runners, then install a lower-version DEB, upgrade to the release, launch and remove it in Debian 12/13 and Ubuntu 22.04/24.04/26.04 containers. Separate native jobs build, upgrade, launch and remove Arch and Arch Linux ARM packages and Fedora 43/44, Rocky Linux 9/10 and openSUSE Leap 16 RPMs. The older-version fixtures reuse the release payload while changing package metadata; they test package-manager transactions, while a future released older binary can test data migration. Flatpak and Snap jobs build and install on each native architecture, run both packaged commands, execute a built-in page, fetch a deterministic loopback page, compile a source file, inspect installed URL-handler metadata and open a visible X11 window with Vulkan selected before removal. The installer workflow mounts the macOS DMG, upgrades its component PKG, installs and removes the Windows MSI after an upgrade, checks the portable ZIP, exercises registered URL delivery, and runs Intel Mac theme, CLI and installer jobs. The tag release workflow runs the same smoke scripts before its draft-publication job can run.

`packaging/verify-system-theme.sh` runs in the native Linux x86_64/ARM64, macOS Apple Silicon/Intel and Windows x64 build jobs, including tag releases. It checks the Linux portal request and response parser, the macOS/Windows window-event source, light/dark/unspecified preference transitions, the browser's actual displayed palette, explicit theme overrides and title-bar behavior. These headless tests use controlled preference reports and do not change a runner's live desktop settings. A customer-device test of live OS setting changes remains separate.

The host on which this guide was written is Arch Linux x86_64. It has `makepkg`, `dpkg-deb`, `repo-add`, and `dpkg-scanpackages`. It does not have Apple's `hdiutil` or a Microsoft SDK link tool. An offline Windows-target Cargo check reached `ring` and then stopped because `lib.exe` was absent; that is a toolchain limitation, not evidence about the Windows application binary. Apple describes native `hdiutil`, code signing and notarization in its [macOS distribution guide](https://developer.apple.com/documentation/xcode/packaging-mac-software-for-distribution). GitHub documents the [native runners](https://docs.github.com/en/actions/reference/runners/github-hosted-runners) used by the release workflow.

## Build on the current machine

Build the two Rust executables and then package them:

```sh
cd f1r3-work/f1r3gaze
cargo build --locked --release -p gaze-shell -p f1r3c
packaging/linux/build.sh --format arch --version 0.1.0 --out dist
packaging/linux/build.sh --format deb --version 0.1.0 --out dist
packaging/linux/build.sh --format tar --version 0.1.0 --out dist
packaging/verify-source.sh
```

`build.sh` accepts `--bin DIR` to package a different native build. It compares the requested version with `[workspace.package]` in Cargo and compares both ELF machine types with the host. It fails rather than labeling a cross-architecture binary as a native package. The Debian dependency list includes the libraries linked by the local binary: glibc, libgcc and fontconfig; window-system and graphics loader dependencies are declared as well. RPMs are compiled on each supported distro baseline because Enterprise Linux 9 has a different library floor from the Debian build host.

The AppImage tool and runtime are fixed to versioned upstream releases and verified against their published SHA-256 digests:

```sh
packaging/linux/fetch-appimagetool.sh dist/appimagetool-tools
APPIMAGETOOL=packaging/linux/appimagetool-wrapper.sh \
APPIMAGETOOL_IMAGE="$PWD/dist/appimagetool-tools/appimagetool-x86_64.AppImage" \
APPIMAGETOOL_RUNTIME_FILE="$PWD/dist/appimagetool-tools/runtime-x86_64" \
packaging/linux/build.sh --format appimage --version 0.1.0 --out dist
```

The wrapper runs the AppImage tool without requiring FUSE on CI. The tool release and runtime checksums are taken from the [appimagetool](https://github.com/AppImage/appimagetool/releases/tag/1.9.1) and [type2 runtime](https://github.com/AppImage/type2-runtime/releases/tag/20251108) release assets.

Inspect the resulting Arch and Debian packages before installing them:

```sh
pacman -Qip dist/f1r3gaze-0.1.0-1-x86_64.pkg.tar.zst
pacman -Qlp dist/f1r3gaze-0.1.0-1-x86_64.pkg.tar.zst
dpkg-deb --field dist/f1r3gaze_0.1.0_amd64.deb
dpkg-deb --contents dist/f1r3gaze_0.1.0_amd64.deb
```

Install local packages with `sudo pacman -U FILE.pkg.tar.zst` on Arch or `sudo apt install ./FILE.deb` on Debian/Ubuntu. For Fedora or Enterprise Linux use `sudo dnf install ./FILE.rpm`; on openSUSE use `sudo zypper install ./FILE.rpm`. These local commands use the package manager's dependency resolver. The tarball and AppImage are portable artifacts and do not create a package-manager upgrade path.

From `f1r3-work/f1r3gaze`, the following commands install a locally built 0.1.0 package for the current CPU. Use a newer version and repeat the install command to upgrade a local DEB, Arch package or RPM. Select the RPM channel that matches the running distribution (`fc43`, `fc44`, `el9`, `el10` or `opensuse16`):

```sh
VERSION=0.1.0
sudo pacman -U "dist/f1r3gaze-${VERSION}-1-$(uname -m).pkg.tar.zst"
f1r3gaze --version
sudo pacman -R f1r3gaze

sudo apt install "./dist/f1r3gaze_${VERSION}_$(dpkg --print-architecture).deb"
f1r3gaze --version
sudo apt remove f1r3gaze

RPM_CHANNEL=fc44  # choose the channel for this host
sudo dnf install "./dist/f1r3gaze-${VERSION}-1.${RPM_CHANNEL}.$(rpm --eval '%{_arch}').rpm"
f1r3gaze --version
sudo dnf remove f1r3gaze

RPM_CHANNEL=opensuse16
sudo zypper install "./dist/f1r3gaze-${VERSION}-1.${RPM_CHANNEL}.$(rpm --eval '%{_arch}').rpm"
f1r3gaze --version
sudo zypper remove f1r3gaze
```

Run only the commands for the current distribution. The sandbox bundles have their own install and removal commands:

```sh
VERSION=0.1.0
flatpak --user install "dist/F1R3Gaze-${VERSION}-$(uname -m).flatpak"
flatpak run --user io.f1r3fly.F1R3Gaze
flatpak --user uninstall io.f1r3fly.F1R3Gaze

SNAP_ARCH=$(dpkg --print-architecture)  # amd64 or arm64
sudo snap install --dangerous "dist/f1r3gaze_${VERSION}_${SNAP_ARCH}.snap"
snap run f1r3gaze.f1r3gaze
sudo snap remove f1r3gaze
```

The Flatpak bundle declares its runtime repository in the bundle; Flatpak may fetch that runtime during installation. The local Snap is unsigned, so `--dangerous` is for development and [does not establish a Store trust or automatic update path](https://snapcraft.io/docs/explanation/snap-development/install-modes/). Store-based installs and refreshes become available after publication.

## Repository and trust flow

`packaging/repository/stage.py` creates an unsigned publication tree in a *fresh* output directory:

```sh
python3 packaging/repository/stage.py --dist dist --out dist/repository-0.1.0
python3 packaging/release_metadata.py catalog --version 0.1.0 --dist dist \
  --base-url https://github.com/F1R3FLY-io/F1R3Gaze/releases/download/v0.1.0 \
  --out dist/catalog.json
```

The tree contains Debian `Packages` and `Release` files, pacman `.db`/`.files` indexes, and RPM `repodata` when `createrepo_c` and RPMs are available. RPM metadata uses gzip explicitly because [createrepo_c supports multiple compression formats](https://man.archlinux.org/man/extra/createrepo_c/createrepo_c.8.en) and its default varies by host. Both branch CI and the release workflow stage the complete DEB, Arch, and RPM matrix with `--version 0.1.0 --require-complete`, then run `packaging/repository/verify-staging.py --tree TREE --version 0.1.0` to check each index references its staged package. The release workflow uploads a compressed copy of the staging tree for review. It is marked `UNSIGNED-STAGING`. It must not be published until its repository signatures, package signatures, public key, fingerprint instructions, and install/update checks are ready. The publication host must be separate from the existing Reach demo Pages site. Staging stops before the tree reaches its configured size budget.

After provisioning an OpenPGP signing key, `packaging/repository/sign-metadata.sh TREE KEY_ID` signs the APT Release file, the pacman database and packages, and each available RPM repository index. It verifies every signature it creates. The staging marker stays in place because production also needs signed RPM packages, key distribution and install/upgrade tests.

### Customer repository configuration after publication

The commands below are templates for the future repository host. Set `BASE` to its HTTPS root and verify the published signing-key fingerprint through an independent channel before importing it. The current unsigned staging tree must never be used as `BASE`.

| Family | Repository path below `BASE` | Add repository after key verification | Update / remove |
| --- | --- | --- | --- |
| Debian and Ubuntu | `apt` | A Deb822 source with `URIs: $BASE/apt`, `Suites: stable`, `Components: main` and `Signed-By: /usr/share/keyrings/f1r3gaze.gpg` | `sudo apt update && sudo apt install f1r3gaze` / `sudo apt remove f1r3gaze` |
| Arch and Arch Linux ARM | `arch/$arch` | A `[f1r3gaze]` entry in `pacman.conf` with `SigLevel = Required DatabaseRequired` and `Server = $BASE/arch/$arch`, after `pacman-key` imports the verified key | `sudo pacman -Syu f1r3gaze` / `sudo pacman -R f1r3gaze` |
| Fedora 43/44 | `rpm/fc$releasever/$basearch` | A DNF `.repo` entry with `gpgcheck=1`, `repo_gpgcheck=1` and the verified key URL | `sudo dnf upgrade f1r3gaze` / `sudo dnf remove f1r3gaze` |
| Enterprise Linux 9/10 | `rpm/el$releasever/$basearch` | The same DNF checks, using the EL channel | `sudo dnf upgrade f1r3gaze` / `sudo dnf remove f1r3gaze` |
| openSUSE Leap 16 | `rpm/opensuse16/$basearch` | A Zypper repository with metadata and package signature checks enabled and the verified key imported | `sudo zypper update f1r3gaze` / `sudo zypper remove f1r3gaze` |

`$arch` is `x86_64` or `aarch64` in the pacman URL; DNF and Zypper expand `$basearch` on the client. APT uses the Debian architecture names `amd64` and `arm64` inside its index. The package repository will publish exact host-specific commands and its fingerprint only after a real key and domain are provisioned.

The draft GitHub release collects artifacts first, generates `catalog.json`, and then generates `SHA256SUMS` and its detached signature when a key is configured. CI also generates candidate Homebrew and WinGet manifests from the actual DMG and MSI bytes. Publish those manifests only after the DMG is notarized and the MSI is signed:

```sh
python3 packaging/release_metadata.py homebrew --version 0.1.0 --dist dist \
  --base-url https://github.com/F1R3FLY-io/F1R3Gaze/releases/download/v0.1.0 \
  --out metadata/Casks/f1r3gaze.rb
python3 packaging/release_metadata.py winget --version 0.1.0 --dist dist \
  --base-url https://github.com/F1R3FLY-io/F1R3Gaze/releases/download/v0.1.0 \
  --out metadata/winget
```

The cask points to the DMG and links `f1r3c` from the installed app. This follows Homebrew's [cask artifact model](https://docs.brew.sh/Cask-Cookbook). The WinGet output is a three-file manifest set matching the [community repository format](https://github.com/microsoft/winget-pkgs/blob/master/.github/instructions/manifests.instructions.md). `packaging/winget/validate.py metadata/winget/F1R3FLY.F1R3Gaze/0.1.0 --dist dist` validates all three files against a pinned copy of [Microsoft's 1.12.0 manifest schemas](https://github.com/microsoft/winget-cli/tree/49d0f8291b40f88744a29ae7ca0d1df962ade537/schemas/JSON/manifests/v1.12.0), checks their shared identity and version, and compares the installer hash with the built MSI. Install `packaging/winget/requirements.txt` first. Microsoft's `winget validate` and its community submission checks remain publication steps. Neither generator submits or publishes anything.

After a signing key and verified fingerprint are published, a customer can check a release download with:

```sh
gpg --verify SHA256SUMS.asc SHA256SUMS
sha256sum -c SHA256SUMS
```

Do not infer trust from a checksum downloaded from the same unauthenticated location. Compare the signing-key fingerprint with an independently published source first. No production signing key or fingerprint has been provisioned yet.

## Native installers and optional stores

The macOS script creates one universal `F1R3Gaze.app` and stages only its two executables, metadata, icon and license. The DMG supports drag-and-drop installation; the component PKG is the building block for a later organization selector. Developer ID Application and Developer ID Installer credentials are separate. The script signs and notarizes both artifacts when those credentials are present. The app registers `f1r3://` and `f1r3h://` as separate URL types in `Info.plist`, following [Apple's URL type contract](https://developer.apple.com/documentation/bundleresources/information-property-list/cfbundleurltypes). Its AppKit delegate receives [`application:openURLs:`](https://developer.apple.com/documentation/AppKit/NSApplicationDelegate/application%28_%3Aopen%3A%29) for cold launches and running instances, accepts only syntactically valid addresses of those two types, and opens each in a browser tab in delivery order. The macOS build jobs compile and test that handler, check the installed bundle's registration, exercise a running-app URL delivery and test cold launch through the system's registered URL association. A Mac App Store variant would need its own sandbox and entitlement review, so it is tracked separately.

The Windows script produces a per-machine MSI and a portable ZIP. On Linux, after installing `cargo-xwin`, PowerShell and the Rust Windows target, run `packaging/windows/build-local.sh 0.1.0 dist` to cross-compile the PE binaries and create the portable ZIP. The local x86_64 ZIP was built and its three files inspected. WiX v4 states it supports Windows only; an MSI build on this Arch host failed in its directory compiler, so the MSI uses a Windows runner. That runner installed a synthetic older MSI, upgraded to the release MSI, delivered a registered URL to the running installed app, removed the MSI and launched both ZIP commands. The MSI registers `f1r3://` and `f1r3h://` and supports silent installation and major upgrades. Stable numeric `X.Y.Z` releases avoid mapping distinct prereleases to the same MSI version. The standalone MSI is the primary channel; Microsoft Store review is a separate follow-up. WinGet publication is also separate from MSI creation.

Flatpak and Snap definitions use the same Gaze and compiler payload. Install `xvfb`, `xauth`, `xdotool` and a Mesa Vulkan driver on the smoke host. On a Linux machine with Flatpak Builder, use `packaging/flatpak/ci-smoke.sh 0.1.0` after building the native executables. It stages the input, creates `dist/F1R3Gaze-0.1.0-$(uname -m).flatpak`, installs it for the current user, exercises the sandbox and removes it. This test adds the Flathub remote for the matching runtime and needs network access. For Snap, prepare `packaging/snap/stage.sh target/release dist/snap-input 0.1.0`, then run Snapcraft 9 or newer on a matching Ubuntu 24.04 builder to pack it; `packaging/snap/ci-smoke.sh FILE.snap 0.1.0` installs, exercises and removes the unsigned snap. Both scripts run the built-in page, fetch a local HTTP page, compile and inspect a source file in the application writable area, check the installed desktop file's custom URL declarations, and open a visible window on an isolated X11 display with the Vulkan backend selected. The Snapcraft 8 `8_core24` OCI image does not support the recipe's GPU extension. The manifests are `packaging/flatpak/io.f1r3fly.F1R3Gaze.json` and `dist/snap-input/snap/snapcraft.yaml`. CI checks a virtual X11 display; real Wayland sessions, hardware GPUs and host desktop URL dispatch still need customer-device validation. Store publication follows those checks.

## Release sequence and pending work

The tag workflow builds native Linux binaries and packages, builds universal macOS and Windows artifacts, checks available metadata, creates a draft release, and produces the cask and WinGet files as review artifacts. The `package-linux-smoke.yml` and `package-installers-smoke.yml` workflows run on `feature/additional-packages` pushes; the installer workflow also runs on `v*` tags. Branch smoke runs cancel an older run for the same workflow and branch; tag runs are retained. Each builds unsigned artifacts on GitHub-hosted runners and performs installation and launch checks. [Fork Linux run 37713789205](https://github.com/dylon/F1R3Gaze/actions/runs/37713789205) passed all 31 package build, upgrade and lifecycle jobs, including both Flatpak and Snap architectures, virtual-display graphics and repository staging; [fork installer run 37713789218](https://github.com/dylon/F1R3Gaze/actions/runs/37713789218) passed all four native installer jobs with theme, PKG and MSI upgrade, and running-app URL checks. The Apple Silicon package runner checks the universal app's two Mach-O slices and tests the installed ARM slice. A separate Intel job downloads and installs that exact universal artifact, checks its native slice, PKG upgrade, running-app and cold URL handling, and gates the release draft; another Intel job checks the native compiler, CLI and system theme. The Windows runner is Windows Server 2022, so Windows 11 customer validation remains separate. Arch Linux ARM uses the third-party `menci/archlinuxarm` image; its package and lifecycle passed on that image. Publishing package repositories, a Homebrew tap, WinGet, Flathub, Snap Store, Microsoft Store or Mac App Store requires external accounts or credentials. Signed install, upgrade and removal tests on customer macOS and Windows 11 machines also remain pending. Each of these is a leaf in the external pgmcp child epic.
