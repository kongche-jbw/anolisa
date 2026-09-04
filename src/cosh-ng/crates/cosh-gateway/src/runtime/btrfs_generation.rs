//! Kernel-backed Btrfs identity compatible with ws-ckpt generation V2.

#[cfg(target_os = "linux")]
mod linux {
    use std::fs::File;
    use std::io;
    use std::mem::{offset_of, size_of};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;

    use sha2::{Digest as _, Sha256};

    const LIVE_GENERATION_DOMAIN: &[u8] = b"ws-ckpt/live-workspace-generation/v2\0";
    const BTRFS_ROOT_INODE: u64 = 256;
    const BTRFS_IOCTL_MAGIC: u8 = 0x94;
    const BTRFS_IOC_FS_INFO_NR: u8 = 31;
    const BTRFS_IOC_GET_SUBVOL_INFO_NR: u8 = 60;
    const BTRFS_FSID_SIZE: usize = 16;
    const BTRFS_UUID_SIZE: usize = 16;
    const BTRFS_VOL_NAME_MAX: usize = 255;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct BtrfsIoctlFsInfoArgs {
        max_id: u64,
        num_devices: u64,
        fsid: [u8; BTRFS_FSID_SIZE],
        nodesize: u32,
        sectorsize: u32,
        clone_alignment: u32,
        csum_type: u16,
        csum_size: u16,
        flags: u64,
        generation: u64,
        metadata_uuid: [u8; BTRFS_FSID_SIZE],
        reserved: [u8; 944],
    }

    impl Default for BtrfsIoctlFsInfoArgs {
        fn default() -> Self {
            Self {
                max_id: 0,
                num_devices: 0,
                fsid: [0; BTRFS_FSID_SIZE],
                nodesize: 0,
                sectorsize: 0,
                clone_alignment: 0,
                csum_type: 0,
                csum_size: 0,
                flags: 0,
                generation: 0,
                metadata_uuid: [0; BTRFS_FSID_SIZE],
                reserved: [0; 944],
            }
        }
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct BtrfsIoctlTimespec {
        sec: u64,
        nsec: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct BtrfsIoctlGetSubvolInfoArgs {
        treeid: u64,
        name: [u8; BTRFS_VOL_NAME_MAX + 1],
        parent_id: u64,
        dirid: u64,
        generation: u64,
        flags: u64,
        uuid: [u8; BTRFS_UUID_SIZE],
        parent_uuid: [u8; BTRFS_UUID_SIZE],
        received_uuid: [u8; BTRFS_UUID_SIZE],
        ctransid: u64,
        otransid: u64,
        stransid: u64,
        rtransid: u64,
        ctime: BtrfsIoctlTimespec,
        otime: BtrfsIoctlTimespec,
        stime: BtrfsIoctlTimespec,
        rtime: BtrfsIoctlTimespec,
        reserved: [u64; 8],
    }

    impl Default for BtrfsIoctlGetSubvolInfoArgs {
        fn default() -> Self {
            Self {
                treeid: 0,
                name: [0; BTRFS_VOL_NAME_MAX + 1],
                parent_id: 0,
                dirid: 0,
                generation: 0,
                flags: 0,
                uuid: [0; BTRFS_UUID_SIZE],
                parent_uuid: [0; BTRFS_UUID_SIZE],
                received_uuid: [0; BTRFS_UUID_SIZE],
                ctransid: 0,
                otransid: 0,
                stransid: 0,
                rtransid: 0,
                ctime: BtrfsIoctlTimespec::default(),
                otime: BtrfsIoctlTimespec::default(),
                stime: BtrfsIoctlTimespec::default(),
                rtime: BtrfsIoctlTimespec::default(),
                reserved: [0; 8],
            }
        }
    }

    nix::ioctl_read!(
        btrfs_ioc_fs_info,
        BTRFS_IOCTL_MAGIC,
        BTRFS_IOC_FS_INFO_NR,
        BtrfsIoctlFsInfoArgs
    );
    nix::ioctl_read!(
        btrfs_ioc_get_subvol_info,
        BTRFS_IOCTL_MAGIC,
        BTRFS_IOC_GET_SUBVOL_INFO_NR,
        BtrfsIoctlGetSubvolInfoArgs
    );

    const _: () = {
        assert!(size_of::<BtrfsIoctlFsInfoArgs>() == 1024);
        assert!(offset_of!(BtrfsIoctlFsInfoArgs, fsid) == 16);
        assert!(offset_of!(BtrfsIoctlGetSubvolInfoArgs, uuid) == 296);
        assert!(
            size_of::<BtrfsIoctlGetSubvolInfoArgs>() == 488
                || size_of::<BtrfsIoctlGetSubvolInfoArgs>() == 504
        );
    };

    pub(crate) fn from_directory(directory: &File) -> io::Result<[u8; 32]> {
        if directory.metadata()?.ino() != BTRFS_ROOT_INODE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "workspace is not a Btrfs subvolume root",
            ));
        }
        let mut fs_info = BtrfsIoctlFsInfoArgs::default();
        // SAFETY: `directory` retains the fd and the argument matches Linux UAPI.
        unsafe { btrfs_ioc_fs_info(directory.as_raw_fd(), &mut fs_info) }
            .map_err(io::Error::from)?;
        let mut subvolume = BtrfsIoctlGetSubvolInfoArgs::default();
        // SAFETY: `directory` retains the fd and the argument matches Linux UAPI.
        unsafe { btrfs_ioc_get_subvol_info(directory.as_raw_fd(), &mut subvolume) }
            .map_err(io::Error::from)?;
        token_from_ids(fs_info.fsid, subvolume.uuid)
    }

    fn token_from_ids(fsid: [u8; 16], uuid: [u8; 16]) -> io::Result<[u8; 32]> {
        if fsid.iter().all(|byte| *byte == 0) || uuid.iter().all(|byte| *byte == 0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Btrfs workspace identity contains a zero UUID",
            ));
        }
        let mut hasher = Sha256::new();
        hasher.update(LIVE_GENERATION_DOMAIN);
        hasher.update(fsid);
        hasher.update(uuid);
        Ok(hasher.finalize().into())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn token_matches_ws_ckpt_fixed_vector() {
            let fsid = [
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff,
            ];
            let uuid = [
                0xff, 0xee, 0xdd, 0xcc, 0xbb, 0xaa, 0x99, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22,
                0x11, 0x00,
            ];

            let actual = token_from_ids(fsid, uuid)
                .unwrap()
                .into_iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();

            assert_eq!(
                actual,
                "57f4a595e2283b4fda2f746d09b21911b6965f72e33441bb78ee7d1f78fcddff"
            );
        }
    }
}

#[cfg(target_os = "linux")]
pub(super) use linux::from_directory;

#[cfg(not(target_os = "linux"))]
pub(super) fn from_directory(_directory: &std::fs::File) -> std::io::Result<[u8; 32]> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Btrfs workspace generation requires Linux",
    ))
}
