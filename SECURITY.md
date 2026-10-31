# Security

Salvage reads raw disks, so please report security problems privately through
[GitHub security advisories](https://github.com/egcax/salvage/security/advisories/new)
rather than public issues.

## Model

- `salvage-engine` starts as root through polkit, opens the device, reads SMART and drops
  all privileges before parsing anything on the disk.
- Only block devices can be opened with privileges.
- File names from a disk are untrusted. They are sanitized and files are created with
  `O_NOFOLLOW | O_EXCL`.
- The "do not ask for a password" setting lets any program running as that user read
  whole disks, including files owned by other users. It is off by default.
- SMART reading is allowed without a password because it exposes only disk health data.
