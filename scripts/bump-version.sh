#!/bin/sh
# Sets the app version everywhere it is defined: package.json, src-tauri/Cargo.toml and
# src-tauri/tauri.conf.json, plus the two lock files that repeat it. The About dialog reads
# package.json. scripts/bump-version.ps1 does the same on Windows.
#
#   sh scripts/bump-version.sh 0.3.0
set -eu

version=${1:-}
# Plain numbers only: the Windows .msi installer accepts no other version.
if ! printf '%s\n' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
  echo "Give the version as three numbers, for example: sh $0 0.3.0" >&2
  exit 1
fi
cd "$(dirname "$0")/.."

# Each pattern ends right before the app's own version, never a dependency's. Every file is
# checked before any is written, so a file that doesn't match leaves all of them as they were.
VERSION=$version perl -e '
  my @edits = (
    ["package.json", qr/^  "version": "/m],
    ["package-lock.json", qr/"name": "tricky3d",\s*"version": "/],
    ["src-tauri/Cargo.toml", qr/^version = "/m],
    ["src-tauri/Cargo.lock", qr/name = "tricky3d"\r?\nversion = "/],
    ["src-tauri/tauri.conf.json", qr/^  "version": "/m],
  );
  my @texts;
  for my $edit (@edits) {
    my ($file, $pattern) = @$edit;
    open my $in, "<:raw", $file or die "$file: $!\n";
    my $text = do { local $/; <$in> };
    close $in;
    $text =~ s/$pattern\K[^"]*/$ENV{VERSION}/g or die "$file: no version line found.\n";
    push @texts, $text;
  }
  for my $i (0 .. $#edits) {
    open my $out, ">:raw", $edits[$i][0] or die "$edits[$i][0]: $!\n";
    print $out $texts[$i];
    close $out or die "$edits[$i][0]: $!\n";
  }
'
echo "Version set to $version. Tag the release as v$version."
