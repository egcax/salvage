# Changelog

## 2.1.1

- Fixed dates on FAT drives and in photo EXIF data, which were shifted by the time zone offset
- exFAT dates now use the UTC offset stored on the disk

## 2.1.0

- New home dashboard with drive cards, usage charts, protection status and recent scans
- Results now have sortable columns, colour-coded file types and a scan summary panel
- History and settings pages redesigned
- Welcome screen removed; the app opens straight on the dashboard
- First public release, with pacman, deb, rpm and AppImage downloads
- AppImage can now scan drives: pkexec gets a copy of the engine outside the FUSE mount

## 2.0.0

- 24 languages, following the system language
- F2FS parser with names and folders; XFS free-space aware signature search
- TRIM pause, emergency rescue, recovery while scanning
- Checksum-verified recovery chance on btrfs
- Two-fragment JPEG reassembly
- Open-and-check validation, duplicates, search inside files, list export
- Disk imaging with bad sector retries and resume; scan resume
- LUKS and LVM volumes, passwordless SMART, optional passwordless scanning
- Speed limit, low priority, sleep prevention, desktop notifications
- Page transitions up to 18 times smoother; worst frame down from 216 ms to 12 ms

## 1.0.0

- First version
