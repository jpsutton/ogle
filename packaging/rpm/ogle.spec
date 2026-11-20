# Binary package from a prebuilt `ogle` (scripts/ci/build-rpm.sh compiles it on the
# target release first). The version comes from Cargo.toml via --define ogle_version.
Name:           ogle
Version:        %{ogle_version}
Release:        1%{?dist}
Summary:        Resource-efficient terminal system monitor
License:        LGPL-3.0-or-later
URL:            https://github.com/jpsutton/ogle
Source0:        ogle
Source1:        README.md
Source2:        LICENSE

%global debug_package %{nil}

%description
ogle shows CPU, memory, swap, load, network, disk I/O, file systems, sensors and
processes on one screen, inspired by Glances, at a small fraction of its CPU and
memory cost.

%prep

%build

%install
install -Dm755 %{SOURCE0} %{buildroot}%{_bindir}/ogle
install -Dm644 %{SOURCE1} %{buildroot}%{_docdir}/ogle/README.md
install -Dm644 %{SOURCE2} %{buildroot}%{_licensedir}/ogle/LICENSE

%files
%{_bindir}/ogle
%doc %{_docdir}/ogle/README.md
%license %{_licensedir}/ogle/LICENSE
