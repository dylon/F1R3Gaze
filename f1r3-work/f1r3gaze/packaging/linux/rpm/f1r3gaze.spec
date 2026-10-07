Name:           f1r3gaze
Version:        %{gaze_version}
Release:        1%{?dist}
Summary:        F1R3Gaze browser with in-process RSpace and f1r3c
License:        Apache-2.0
URL:            https://github.com/F1R3FLY-io/F1R3Gaze
AutoReqProv:    yes
%if 0%{?gaze_suse}
Requires:       libfontconfig1
Requires:       libxkbcommon0
Requires:       libvulkan1
%else
Requires:       fontconfig
Requires:       libxkbcommon
Requires:       vulkan-loader
%endif

%description
F1R3Gaze renders sites with an in-process RSpace. F1R3Node is an optional,
separately deployed service. Embers is not bundled.

%prep

%build

%install
install -Dm755 "%{gaze_bin}/f1r3gaze" "%{buildroot}%{_bindir}/f1r3gaze"
install -Dm755 "%{gaze_bin}/f1r3c" "%{buildroot}%{_bindir}/f1r3c"
install -Dm644 "%{gaze_project}/packaging/linux/f1r3gaze.desktop" "%{buildroot}%{_datadir}/applications/f1r3gaze.desktop"
install -Dm644 "%{gaze_project}/../../LICENSE" "%{buildroot}%{_datadir}/licenses/f1r3gaze/LICENSE"
for size in 16 32 48 64 128 256 512; do
  install -Dm644 "%{gaze_project}/packaging/icons/$size.png" "%{buildroot}%{_datadir}/icons/hicolor/${size}x${size}/apps/f1r3gaze.png"
done
install -Dm644 "%{gaze_project}/packaging/icons/f1r3gaze.svg" "%{buildroot}%{_datadir}/icons/hicolor/scalable/apps/f1r3gaze.svg"

%files
%{_bindir}/f1r3gaze
%{_bindir}/f1r3c
%{_datadir}/applications/f1r3gaze.desktop
%license %{_datadir}/licenses/f1r3gaze/LICENSE
%{_datadir}/icons/hicolor/*/apps/f1r3gaze.*
