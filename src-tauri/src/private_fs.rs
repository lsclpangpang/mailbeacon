//! Restrict private local storage to the owning desktop user.

#[cfg(windows)]
use anyhow::Context;
use anyhow::{Result, bail};
use std::{fs, path::Path};

/// Treat Windows junctions and other reparse points as links too.
pub fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    metadata.file_type().is_symlink()
}

/// Create an application-private directory, or tighten an existing one's ACL.
pub fn create_private_dir(path: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    protect(path, true)
}

/// Restrict an existing regular file; reject link/reparse redirection.
pub fn protect_file(path: &Path) -> Result<()> {
    protect(path, false)
}

fn protect(path: &Path, directory: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if is_link(&metadata) || metadata.is_dir() != directory || (!directory && !metadata.is_file()) {
        bail!("本地私有存储路径无效");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }),
        )?;
    }
    #[cfg(windows)]
    windows::protect(path, directory).context("无法限制本地私有存储权限")?;
    Ok(())
}

#[cfg(all(test, windows))]
pub(crate) fn assert_private_acl(path: &Path) {
    windows::assert_private_acl(path);
}

#[cfg(all(test, windows))]
pub(crate) fn test_junction(target: &Path, link: &Path) {
    use std::os::windows::process::CommandExt;
    // Junctions work without Administrator / Developer Mode, unlike symlink_dir.
    // Windows file names cannot contain a quote, and quoting protects spaces and &.
    let output = std::process::Command::new("cmd.exe")
        .raw_arg("/D /C")
        .raw_arg(format!(
            "mklink /J \"{}\" \"{}\"",
            link.display(),
            target.display()
        ))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "junction creation failed: {:?}",
        output
    );
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::{
        ffi::c_void,
        io,
        os::windows::{
            fs::OpenOptionsExt,
            io::{AsRawHandle, FromRawHandle, OwnedHandle},
        },
        ptr::{null, null_mut},
    };
    use windows_sys::Win32::{
        Foundation::{ERROR_SUCCESS, LocalFree},
        Security::{
            ACL,
            Authorization::{
                EXPLICIT_ACCESS_W, SE_FILE_OBJECT, SET_ACCESS, SetEntriesInAclW, SetSecurityInfo,
                TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_IS_WELL_KNOWN_GROUP, TRUSTEE_W,
            },
            CONTAINER_INHERIT_ACE, CreateWellKnownSid, DACL_SECURITY_INFORMATION,
            GetTokenInformation, OBJECT_INHERIT_ACE, PROTECTED_DACL_SECURITY_INFORMATION,
            SECURITY_MAX_SID_SIZE, TOKEN_QUERY, TOKEN_USER, TokenUser, WinLocalSystemSid,
        },
        Storage::FileSystem::{
            FILE_ALL_ACCESS, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_READ_ATTRIBUTES, READ_CONTROL, WRITE_DAC,
        },
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };

    struct LocalAllocation(*mut c_void);
    impl Drop for LocalAllocation {
        fn drop(&mut self) {
            unsafe {
                LocalFree(self.0);
            }
        }
    }

    // usize storage keeps TOKEN_USER / SID buffers correctly aligned.
    fn user_token() -> io::Result<Vec<usize>> {
        let mut token = null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        let mut needed = 0;
        unsafe {
            GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut needed);
        }
        if needed < std::mem::size_of::<TOKEN_USER>() as u32 {
            return Err(io::Error::last_os_error());
        }
        let mut buffer = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
        if unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(buffer)
    }

    fn system_sid() -> io::Result<Vec<usize>> {
        let mut length = SECURITY_MAX_SID_SIZE;
        let mut buffer = vec![0usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
        if unsafe {
            CreateWellKnownSid(
                WinLocalSystemSid,
                null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut length,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(buffer)
    }

    fn open(path: &Path, writable_acl: bool) -> io::Result<fs::File> {
        fs::OpenOptions::new()
            .access_mode(
                READ_CONTROL | FILE_READ_ATTRIBUTES | if writable_acl { WRITE_DAC } else { 0 },
            )
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
    }

    pub(super) fn protect(path: &Path, directory: bool) -> Result<()> {
        let file = open(path, true)?;
        let metadata = file.metadata()?;
        if is_link(&metadata)
            || metadata.is_dir() != directory
            || (!directory && !metadata.is_file())
        {
            bail!("本地私有存储路径在设置权限时被替换");
        }
        let user = user_token()?;
        let user_sid = unsafe { (*(user.as_ptr().cast::<TOKEN_USER>())).User.Sid };
        let mut system = system_sid()?;
        let inheritance = if directory {
            OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
        } else {
            0
        };
        let grant = |sid: *mut c_void, kind| EXPLICIT_ACCESS_W {
            grfAccessPermissions: FILE_ALL_ACCESS,
            grfAccessMode: SET_ACCESS,
            grfInheritance: inheritance,
            Trustee: TRUSTEE_W {
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: kind,
                ptstrName: sid.cast(),
                ..Default::default()
            },
        };
        let entries = [
            grant(user_sid, TRUSTEE_IS_USER),
            grant(system.as_mut_ptr().cast(), TRUSTEE_IS_WELL_KNOWN_GROUP),
        ];
        let mut acl: *mut ACL = null_mut();
        let error =
            unsafe { SetEntriesInAclW(entries.len() as u32, entries.as_ptr(), null(), &mut acl) };
        if error != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(error as i32).into());
        }
        let _acl = LocalAllocation(acl.cast());
        // Start from an empty ACL and protect it: inherited Users/Everyone access
        // must not survive when restoring files from a broader-permission folder.
        let error = unsafe {
            SetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                acl,
                null(),
            )
        };
        if error != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(error as i32).into());
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn assert_private_acl(path: &Path) {
        use windows_sys::Win32::Security::{
            ACCESS_ALLOWED_ACE, Authorization::GetSecurityInfo, EqualSid, GetAce,
            GetSecurityDescriptorControl, SE_DACL_PROTECTED,
        };
        let file = open(path, false).unwrap();
        let mut acl = null_mut();
        let mut descriptor = null_mut();
        assert_eq!(
            unsafe {
                GetSecurityInfo(
                    file.as_raw_handle(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    &mut acl,
                    null_mut(),
                    &mut descriptor,
                )
            },
            ERROR_SUCCESS
        );
        let _descriptor = LocalAllocation(descriptor);
        assert!(!acl.is_null());
        let mut control = 0;
        let mut revision = 0;
        assert_ne!(
            unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) },
            0
        );
        assert_ne!(control & SE_DACL_PROTECTED, 0);
        assert_eq!(unsafe { (*acl).AceCount }, 2);
        let user = user_token().unwrap();
        let user_sid = unsafe { (*(user.as_ptr().cast::<TOKEN_USER>())).User.Sid };
        let mut system = system_sid().unwrap();
        let mut found = [false; 2];
        for index in 0..2 {
            let mut ace = null_mut();
            assert_ne!(unsafe { GetAce(acl, index, &mut ace) }, 0);
            let ace = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
            assert_eq!(ace.Header.AceType, 0); // ACCESS_ALLOWED_ACE_TYPE
            assert_eq!(ace.Mask, FILE_ALL_ACCESS);
            let sid = (&ace.SidStart as *const u32).cast_mut().cast();
            let is_user = unsafe { EqualSid(sid, user_sid) } != 0;
            let is_system = unsafe { EqualSid(sid, system.as_mut_ptr().cast()) } != 0;
            assert!(
                is_user || is_system,
                "unexpected account retained access to the local key"
            );
            found[0] |= is_user;
            found[1] |= is_system;
        }
        assert_eq!(found, [true, true]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_directory_and_file_remain_readable_by_current_user() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("密钥 secrets");
        create_private_dir(&dir).unwrap();
        let path = dir.join("secret.key");
        std::fs::write(&path, b"private").unwrap();
        protect_file(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"private");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        #[cfg(windows)]
        {
            windows::assert_private_acl(&dir);
            windows::assert_private_acl(&path);
        }
    }

    #[cfg(unix)]
    #[test]
    fn private_directory_rejects_symlink_without_changing_target_permissions() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        let link = root.path().join("link");
        symlink(&target, &link).unwrap();
        assert!(create_private_dir(&link).is_err());
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[cfg(windows)]
    #[test]
    fn private_directory_rejects_junction_without_writing_through_it() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("outside");
        std::fs::create_dir(&target).unwrap();
        let link = root.path().join("secrets");
        test_junction(&target, &link);
        assert!(is_link(&std::fs::symlink_metadata(&link).unwrap()));
        assert!(create_private_dir(&link).is_err());
        assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);
    }
}
