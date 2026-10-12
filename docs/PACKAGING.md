# Building distributable packages / 构建分发包

EndlessVibe is a **single-owner Rust MCP service with an embedded Web UI**, not
an Electron/Qt application. Its HTML/CSS/JavaScript and favicon are compiled
into the executable. Packaging the binary **must not package** the user's
config, Owner Key, tokens, workspaces, or SQLite state.

Run the commands below from the repository root. The scripts read the
package version from `Cargo.toml` automatically.

## Windows: a single endlessvibe.exe / Windows 单文件

Build on **Windows 10/11 x64** with the stable
`x86_64-pc-windows-msvc` target and Visual Studio C++ Build Tools:

```powershell
.\deploy\windows\Package.ps1

# Already cached dependencies:
.\deploy\windows\Package.ps1 -Offline

# Recheck a previously built target/package-windows release:
.\deploy\windows\Package.ps1 -SkipBuild
```

**The distributable is exactly:**

```text
dist\windows\endlessvibe.exe
```

Copy **only this EXE** to another supported Windows computer; no sidecar
`web/`, `config.toml`, `owner.key`, DLLs or separate runtime installer is
included. Each user has private configuration and state under
`%APPDATA%\EndlessVibe` and `%LOCALAPPDATA%\EndlessVibe`.

```powershell
# On the recipient's Windows computer, from the EXE's directory:
.\endlessvibe.exe --init

# If the binary's --help advertises them, use --init --cli or --init --web.
# After initializing (and exiting the Web wizard, if selected):
.\endlessvibe.exe
```

The script:
1. Runs `cargo build --release --locked --target x86_64-pc-windows-msvc`
   with `-C target-feature=+crt-static`. The MSVC CRT is linked statically
   for Rust and bundled native libraries.
2. Tests the built EXE using `--version` and `--help`.
3. Audits the AMD64 PE32+ imports and rejects delay-load or non-system DLLs.
4. Copies only the verified EXE into `dist/windows` and prints SHA-256.

**Single file does not mean zero operating-system dependencies.** Standard
Windows system DLLs are required. User-requested compilers/Git need to be
installed on the destination if corresponding tools should work.

The existing `deploy/windows/EndlessVibe.ps1 -Action Update` script is
**separate**: it manages a local running service. `Package.ps1` never
installs, starts, stops, deploys services, or distributes configuration.

If offline Cargo cannot find a locked Windows dependency, fetch first on a
**trusted Windows build machine**, then retry:

```powershell
cargo fetch --locked --target x86_64-pc-windows-msvc
.\deploy\windows\Package.ps1 -Offline
```

Native builds and clean-machine tests are required before declaring Windows
release compatibility. A Linux build cannot establish that claim.

## Linux: one AppImage / Linux 单文件

Linux packaging requires Bash, Cargo, `readelf`, and the **official
appimagetool** (only on the build machine). On x86_64 or aarch64:

```bash
# Obtain appimagetool yourself from its official releases:
# https://github.com/AppImage/appimagetool/releases/continuous
chmod +x /path/to/appimagetool-x86_64.AppImage

./deploy/linux/package-appimage.sh \
  --appimagetool /path/to/appimagetool-x86_64.AppImage

# If Cargo dependencies and the Type 2 runtime are already cached:
./deploy/linux/package-appimage.sh --offline \
  --appimagetool /path/to/appimagetool-x86_64.AppImage \
  --runtime-file /path/to/type2-runtime-x86_64
```

On `aarch64`, use the matching `appimagetool-aarch64.AppImage` and
`runtime-aarch64` rather than the x86_64 example paths.

Output:

```text
dist/linux/EndlessVibe-VERSION-x86_64.AppImage
# Or: EndlessVibe-VERSION-aarch64.AppImage
```

The script builds a standard AppDir with `AppRun`,
`endlessvibe.desktop` (`Terminal=true`), a PNG icon, the MIT license, and
`usr/bin/endlessvibe`. The executable contains the Setup/Dashboard Web UI;
`AppRun` forwards all command-line arguments. Unexpected ELF dependencies
are rejected before packaging. The builder runs `--version` and `--help`
on the resulting AppImage; only the AppImage is distributed.

```bash
chmod +x ./dist/linux/EndlessVibe-*-x86_64.AppImage
./dist/linux/EndlessVibe-*-x86_64.AppImage --init

# Normal start after initialization:
./dist/linux/EndlessVibe-*-x86_64.AppImage

# Type 2 runtime fallback on hosts without FUSE:
APPIMAGE_EXTRACT_AND_RUN=1 ./dist/linux/EndlessVibe-*-x86_64.AppImage --init
```

A single-file AppImage is **not universal Linux compatibility**: the program
requires a compatible kernel and glibc (build on an older supported distro
for wider portability). Sandboxed command execution needs the host's
Bubblewrap/user namespaces, and project actions may require installed tools
such as Git and Cargo. Type 2 AppImage runtimes may need FUSE 2 or
extract-and-run support.

To reuse an existing `target/release/endlessvibe` (version checked):

```bash
./deploy/linux/package-appimage.sh --skip-build \
  --appimagetool /path/to/appimagetool-x86_64.AppImage
```

Use `--output-dir` for a different destination or `--no-smoke` only when the
builder cannot execute a valid AppImage. Set `APPIMAGETOOL_RUNTIME_FILE` or
`--runtime-file` for offline or reproducible builds; newer appimagetool
versions may otherwise fetch the Type 2 runtime.

## Release checklist / 发布检查

- Build and test on each **native target OS and architecture**. Linux tests
  cannot prove Windows packaging.
- Smoke-test `--version`, `--help`, `--init` and a normal restart on a clean
  destination. Public MCP/OAuth requires separate connection verification.
- Compare the output SHA-256 with the copied/downloaded binary.
- **Never distribute** `owner.key`, OAuth tokens, `.env`, `config.toml`,
  private state, SQLite databases or local credentials.
- Never expose Dashboard/setup port `127.0.0.1:20001`.
- Packaging scripts do not Git commit/tag/push or deploy services.

References: [AppDir specification](https://docs.appimage.org/reference/appdir.html),
[appimagetool](https://github.com/AppImage/appimagetool),
[AppImage FUSE troubleshooting](https://docs.appimage.org/user-guide/troubleshooting/fuse.html).
