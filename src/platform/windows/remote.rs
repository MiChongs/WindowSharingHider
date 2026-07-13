use std::collections::HashSet;
use std::ffi::c_void;

use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::System::SystemInformation::IMAGE_FILE_MACHINE;
use windows::Win32::System::Threading::IsWow64Process2;

use crate::model::{OperationError, OperationStage, WindowHandle};

use super::resources::{OwnedHandle, last_error};

const IMAGE_FILE_MACHINE_UNKNOWN: u16 = 0x0000;
const IMAGE_FILE_MACHINE_I386: u16 = 0x014c;
const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;
const IMAGE_NT_SIGNATURE: u32 = 0x0000_4550;
const PE32_MAGIC: u16 = 0x010b;
const PE32_PLUS_MAGIC: u16 = 0x020b;
const MAX_EXPORT_ENTRIES: u32 = 1_000_000;
const MAX_EXPORT_NAME_BYTES: usize = 512;
const MAX_FORWARD_DEPTH: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TargetArchitecture {
    X86,
    X64,
}

impl TargetArchitecture {
    pub(crate) fn detect(process: &OwnedHandle) -> Result<Self, OperationError> {
        let mut process_machine = IMAGE_FILE_MACHINE(IMAGE_FILE_MACHINE_UNKNOWN);
        let mut native_machine = IMAGE_FILE_MACHINE(IMAGE_FILE_MACHINE_UNKNOWN);
        // SAFETY: Both output pointers are valid and process is a live owned HANDLE.
        if unsafe {
            IsWow64Process2(
                process.as_raw(),
                &mut process_machine,
                Some(&mut native_machine),
            )
        }
        .is_err()
        {
            return Err(last_error(
                OperationStage::DetectArchitecture,
                "IsWow64Process2 无法识别目标进程架构",
            ));
        }

        let effective_machine = if process_machine.0 == IMAGE_FILE_MACHINE_UNKNOWN {
            native_machine.0
        } else {
            process_machine.0
        };
        match effective_machine {
            IMAGE_FILE_MACHINE_I386 => Ok(Self::X86),
            IMAGE_FILE_MACHINE_AMD64 => {
                if cfg!(target_pointer_width = "32") {
                    Err(OperationError::new(
                        OperationStage::DetectArchitecture,
                        "32 位版本不能安全控制 64 位目标；请使用 x64 构建",
                    ))
                } else {
                    Ok(Self::X64)
                }
            }
            other => Err(OperationError::new(
                OperationStage::DetectArchitecture,
                format!("不支持的目标机器类型 0x{other:04X}"),
            )),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RemoteModule {
    pub name: String,
    pub base: u64,
    pub size: u32,
}

impl RemoteModule {
    fn contains_range(&self, address: u64, length: usize) -> bool {
        let module_end = self.base.saturating_add(u64::from(self.size));
        let read_end = address.saturating_add(length as u64);
        address >= self.base && read_end >= address && read_end <= module_end
    }
}

pub(crate) trait RemoteMemoryReader {
    fn read_exact(&self, address: u64, buffer: &mut [u8]) -> Result<(), OperationError>;
}

pub(crate) struct ProcessMemoryReader<'a> {
    process: &'a OwnedHandle,
}

impl<'a> ProcessMemoryReader<'a> {
    pub(crate) const fn new(process: &'a OwnedHandle) -> Self {
        Self { process }
    }
}

impl RemoteMemoryReader for ProcessMemoryReader<'_> {
    fn read_exact(&self, address: u64, buffer: &mut [u8]) -> Result<(), OperationError> {
        let mut bytes_read = 0usize;
        // SAFETY: The destination slice is writable for buffer.len() bytes. The
        // source address is validated by ReadProcessMemory in the target process.
        let result = unsafe {
            ReadProcessMemory(
                self.process.as_raw(),
                address as *const c_void,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                Some(&mut bytes_read),
            )
        };
        if result.is_err() || bytes_read != buffer.len() {
            return Err(last_error(
                OperationStage::ReadRemoteImage,
                format!(
                    "远程地址 0x{address:X} 需要 {} 字节，实际读取 {bytes_read} 字节",
                    buffer.len()
                ),
            ));
        }
        Ok(())
    }
}

pub(crate) struct ExportResolver<'a, R: RemoteMemoryReader> {
    reader: &'a R,
    modules: &'a [RemoteModule],
}

impl<'a, R: RemoteMemoryReader> ExportResolver<'a, R> {
    pub(crate) const fn new(reader: &'a R, modules: &'a [RemoteModule]) -> Self {
        Self { reader, modules }
    }

    pub(crate) fn resolve(&self, module_name: &str, symbol: &str) -> Result<u64, OperationError> {
        let mut visited = HashSet::new();
        self.resolve_inner(module_name, symbol, 0, &mut visited)
    }

    fn resolve_inner(
        &self,
        module_name: &str,
        symbol: &str,
        depth: usize,
        visited: &mut HashSet<String>,
    ) -> Result<u64, OperationError> {
        if depth > MAX_FORWARD_DEPTH {
            return Err(OperationError::new(
                OperationStage::ResolveExport,
                "远程导出转发层级超过安全上限",
            ));
        }

        let module = self.find_module(module_name).ok_or_else(|| {
            OperationError::new(
                OperationStage::ResolveExport,
                format!("目标进程未加载模块 {module_name}"),
            )
        })?;
        let visit_key = format!("{}!{}", normalize_module_name(&module.name), symbol);
        if !visited.insert(visit_key) {
            return Err(OperationError::new(
                OperationStage::ResolveExport,
                "远程导出转发形成循环",
            ));
        }

        let nt_offset = u64::from(self.read_u32(module, module.base + 0x3c)?);
        let nt_headers = module.base.checked_add(nt_offset).ok_or_else(|| {
            OperationError::new(OperationStage::ResolveExport, "NT header 地址溢出")
        })?;
        if self.read_u32(module, nt_headers)? != IMAGE_NT_SIGNATURE {
            return Err(OperationError::new(
                OperationStage::ResolveExport,
                format!("{} 不包含有效 PE signature", module.name),
            ));
        }

        let optional_header = nt_headers + 0x18;
        let magic = self.read_u16(module, optional_header)?;
        let data_directory_offset = match magic {
            PE32_MAGIC => 0x60,
            PE32_PLUS_MAGIC => 0x70,
            _ => {
                return Err(OperationError::new(
                    OperationStage::ResolveExport,
                    format!("{} 使用未知 optional header 0x{magic:04X}", module.name),
                ));
            }
        };
        let export_rva = self.read_u32(module, optional_header + data_directory_offset)?;
        let export_size = self.read_u32(module, optional_header + data_directory_offset + 4)?;
        if export_rva == 0 || export_size < 40 {
            return Err(OperationError::new(
                OperationStage::ResolveExport,
                format!("{} 没有有效导出目录", module.name),
            ));
        }

        let export_address = checked_rva(module, export_rva, 40)?;
        let number_of_functions = self.read_u32(module, export_address + 20)?;
        let number_of_names = self.read_u32(module, export_address + 24)?;
        if number_of_functions == 0
            || number_of_functions > MAX_EXPORT_ENTRIES
            || number_of_names > MAX_EXPORT_ENTRIES
        {
            return Err(OperationError::new(
                OperationStage::ResolveExport,
                "远程导出数量无效或超过安全上限",
            ));
        }

        let functions_rva = self.read_u32(module, export_address + 28)?;
        let names_rva = self.read_u32(module, export_address + 32)?;
        let ordinals_rva = self.read_u32(module, export_address + 36)?;
        let functions = checked_rva(
            module,
            functions_rva,
            usize::try_from(number_of_functions)
                .unwrap_or(usize::MAX)
                .saturating_mul(4),
        )?;
        let names = checked_rva(
            module,
            names_rva,
            usize::try_from(number_of_names)
                .unwrap_or(usize::MAX)
                .saturating_mul(4),
        )?;
        let ordinals = checked_rva(
            module,
            ordinals_rva,
            usize::try_from(number_of_names)
                .unwrap_or(usize::MAX)
                .saturating_mul(2),
        )?;

        for index in 0..number_of_names {
            let name_rva = self.read_u32(module, names + u64::from(index) * 4)?;
            let name_address = checked_rva(module, name_rva, 1)?;
            if self.read_c_string(module, name_address)? != symbol {
                continue;
            }

            let ordinal_index = u32::from(self.read_u16(module, ordinals + u64::from(index) * 2)?);
            if ordinal_index >= number_of_functions {
                return Err(OperationError::new(
                    OperationStage::ResolveExport,
                    format!("导出 {symbol} 的 ordinal 超出函数表"),
                ));
            }
            let function_rva = self.read_u32(module, functions + u64::from(ordinal_index) * 4)?;

            let export_end = export_rva.checked_add(export_size).ok_or_else(|| {
                OperationError::new(OperationStage::ResolveExport, "导出目录范围溢出")
            })?;
            if function_rva >= export_rva && function_rva < export_end {
                let forwarder_address = checked_rva(module, function_rva, 1)?;
                let forwarder = self.read_c_string(module, forwarder_address)?;
                let (next_module, next_symbol) = forwarder.rsplit_once('.').ok_or_else(|| {
                    OperationError::new(
                        OperationStage::ResolveExport,
                        format!("无效的 forwarded export：{forwarder}"),
                    )
                })?;
                if next_symbol.starts_with('#') {
                    return Err(OperationError::new(
                        OperationStage::ResolveExport,
                        "不支持按 ordinal 转发的目标导出",
                    ));
                }
                return self.resolve_inner(next_module, next_symbol, depth + 1, visited);
            }

            return Ok(checked_rva(module, function_rva, 1)?);
        }

        Err(OperationError::new(
            OperationStage::ResolveExport,
            format!("{} 中未找到导出 {symbol}", module.name),
        ))
    }

    fn find_module(&self, name: &str) -> Option<&RemoteModule> {
        let expected = normalize_module_name(name);
        self.modules
            .iter()
            .find(|module| normalize_module_name(&module.name) == expected)
    }

    fn read_u16(&self, module: &RemoteModule, address: u64) -> Result<u16, OperationError> {
        let mut bytes = [0u8; 2];
        self.read_bounded(module, address, &mut bytes)?;
        Ok(u16::from_le_bytes(bytes))
    }

    fn read_u32(&self, module: &RemoteModule, address: u64) -> Result<u32, OperationError> {
        let mut bytes = [0u8; 4];
        self.read_bounded(module, address, &mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn read_bounded(
        &self,
        module: &RemoteModule,
        address: u64,
        buffer: &mut [u8],
    ) -> Result<(), OperationError> {
        if !module.contains_range(address, buffer.len()) {
            return Err(OperationError::new(
                OperationStage::ResolveExport,
                format!(
                    "读取范围 0x{address:X}+{} 超出模块 {}",
                    buffer.len(),
                    module.name
                ),
            ));
        }
        self.reader.read_exact(address, buffer)
    }

    fn read_c_string(&self, module: &RemoteModule, address: u64) -> Result<String, OperationError> {
        let mut bytes = Vec::with_capacity(64);
        for offset in 0..MAX_EXPORT_NAME_BYTES {
            let mut byte = [0u8; 1];
            self.read_bounded(module, address + offset as u64, &mut byte)?;
            if byte[0] == 0 {
                return String::from_utf8(bytes).map_err(|_| {
                    OperationError::new(
                        OperationStage::ResolveExport,
                        "远程导出名称不是有效 UTF-8/ASCII",
                    )
                });
            }
            bytes.push(byte[0]);
        }
        Err(OperationError::new(
            OperationStage::ResolveExport,
            "远程导出名称超过安全长度上限",
        ))
    }
}

fn normalize_module_name(name: &str) -> String {
    let mut normalized = name.to_ascii_lowercase();
    if !normalized.ends_with(".dll") {
        normalized.push_str(".dll");
    }
    normalized
}

fn checked_rva(module: &RemoteModule, rva: u32, length: usize) -> Result<u64, OperationError> {
    let address = module
        .base
        .checked_add(u64::from(rva))
        .ok_or_else(|| OperationError::new(OperationStage::ResolveExport, "RVA 地址计算溢出"))?;
    if !module.contains_range(address, length) {
        return Err(OperationError::new(
            OperationStage::ResolveExport,
            format!("RVA 0x{rva:X}+{length} 超出模块 {}", module.name),
        ));
    }
    Ok(address)
}

pub(crate) fn build_call_stub(
    window: WindowHandle,
    affinity: u32,
    function_address: u64,
    architecture: TargetArchitecture,
) -> Result<Vec<u8>, OperationError> {
    match architecture {
        TargetArchitecture::X86 => {
            let hwnd = u32::try_from(window.raw()).map_err(|_| {
                OperationError::new(
                    OperationStage::DetectArchitecture,
                    "窗口句柄不能表示为 x86 指针",
                )
            })?;
            let function = u32::try_from(function_address).map_err(|_| {
                OperationError::new(
                    OperationStage::DetectArchitecture,
                    "函数地址不能表示为 x86 指针",
                )
            })?;
            let mut stub = Vec::with_capacity(18);
            stub.push(0x68);
            stub.extend_from_slice(&affinity.to_le_bytes());
            stub.push(0x68);
            stub.extend_from_slice(&hwnd.to_le_bytes());
            stub.push(0xB8);
            stub.extend_from_slice(&function.to_le_bytes());
            stub.extend_from_slice(&[0xFF, 0xD0, 0xC3]);
            Ok(stub)
        }
        TargetArchitecture::X64 => {
            let hwnd = u64::try_from(window.raw()).map_err(|_| {
                OperationError::new(
                    OperationStage::ValidateWindow,
                    "窗口句柄不能表示为无符号 x64 指针",
                )
            })?;
            let mut stub = Vec::with_capacity(38);
            stub.extend_from_slice(&[0x48, 0x83, 0xEC, 0x28]);
            stub.extend_from_slice(&[0x48, 0xB9]);
            stub.extend_from_slice(&hwnd.to_le_bytes());
            stub.push(0xBA);
            stub.extend_from_slice(&affinity.to_le_bytes());
            stub.extend_from_slice(&[0x48, 0xB8]);
            stub.extend_from_slice(&function_address.to_le_bytes());
            stub.extend_from_slice(&[0xFF, 0xD0]);
            stub.extend_from_slice(&[0x48, 0x83, 0xC4, 0x28, 0xC3]);
            Ok(stub)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: u64 = 0x1000_0000;
    const SIZE: usize = 0x1000;

    struct FixtureReader {
        base: u64,
        bytes: Vec<u8>,
    }

    impl RemoteMemoryReader for FixtureReader {
        fn read_exact(&self, address: u64, buffer: &mut [u8]) -> Result<(), OperationError> {
            let start = usize::try_from(address.saturating_sub(self.base)).map_err(|_| {
                OperationError::new(OperationStage::ReadRemoteImage, "fixture 地址无效")
            })?;
            let end = start.saturating_add(buffer.len());
            let source = self.bytes.get(start..end).ok_or_else(|| {
                OperationError::new(OperationStage::ReadRemoteImage, "fixture 读取越界")
            })?;
            buffer.copy_from_slice(source);
            Ok(())
        }
    }

    fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn fixture(
        module_name: &str,
        symbol: &str,
        function_rva: u32,
    ) -> (FixtureReader, RemoteModule) {
        let mut bytes = vec![0u8; SIZE];
        put_u32(&mut bytes, 0x3c, 0x80);
        put_u32(&mut bytes, 0x80, IMAGE_NT_SIGNATURE);
        put_u16(&mut bytes, 0x98, PE32_PLUS_MAGIC);
        put_u32(&mut bytes, 0x108, 0x200);
        put_u32(&mut bytes, 0x10c, 0x100);
        put_u32(&mut bytes, 0x200 + 20, 1);
        put_u32(&mut bytes, 0x200 + 24, 1);
        put_u32(&mut bytes, 0x200 + 28, 0x300);
        put_u32(&mut bytes, 0x200 + 32, 0x310);
        put_u32(&mut bytes, 0x200 + 36, 0x320);
        put_u32(&mut bytes, 0x300, function_rva);
        put_u32(&mut bytes, 0x310, 0x330);
        put_u16(&mut bytes, 0x320, 0);
        bytes[0x330..0x330 + symbol.len()].copy_from_slice(symbol.as_bytes());
        bytes[0x330 + symbol.len()] = 0;
        (
            FixtureReader { base: BASE, bytes },
            RemoteModule {
                name: module_name.into(),
                base: BASE,
                size: SIZE as u32,
            },
        )
    }

    #[test]
    fn resolves_named_export_from_bounded_pe_image() {
        let (reader, module) = fixture("user32.dll", "SetWindowDisplayAffinity", 0x500);
        let resolver = ExportResolver::new(&reader, std::slice::from_ref(&module));
        assert_eq!(
            resolver
                .resolve("USER32", "SetWindowDisplayAffinity")
                .unwrap(),
            BASE + 0x500
        );
    }

    #[test]
    fn resolves_forwarded_export_with_depth_limit() {
        let (mut first_reader, first) = fixture("user32.dll", "SetWindowDisplayAffinity", 0x280);
        let forwarder = b"kernelbase.SetWindowDisplayAffinity\0";
        first_reader.bytes[0x280..0x280 + forwarder.len()].copy_from_slice(forwarder);

        let (second_reader, second) = fixture("kernelbase.dll", "SetWindowDisplayAffinity", 0x600);
        let mut combined = first_reader.bytes;
        combined.extend_from_slice(&second_reader.bytes);
        let reader = FixtureReader {
            base: BASE,
            bytes: combined,
        };
        let second = RemoteModule {
            base: BASE + SIZE as u64,
            ..second
        };
        let modules = [first, second];
        let resolver = ExportResolver::new(&reader, &modules);
        assert_eq!(
            resolver
                .resolve("user32.dll", "SetWindowDisplayAffinity")
                .unwrap(),
            BASE + SIZE as u64 + 0x600
        );
    }

    #[test]
    fn rejects_out_of_range_function_rva() {
        let (reader, module) = fixture("user32.dll", "Target", 0x2000);
        let resolver = ExportResolver::new(&reader, std::slice::from_ref(&module));
        let error = resolver.resolve("user32", "Target").unwrap_err();
        assert_eq!(error.stage, OperationStage::ResolveExport);
    }

    #[test]
    fn rejects_bad_ordinal() {
        let (mut reader, module) = fixture("user32.dll", "Target", 0x500);
        put_u16(&mut reader.bytes, 0x320, 3);
        let resolver = ExportResolver::new(&reader, std::slice::from_ref(&module));
        assert!(resolver.resolve("user32", "Target").is_err());
    }

    #[test]
    fn generates_exact_x86_stub() {
        let stub = build_call_stub(
            WindowHandle::new(0x1122_3344),
            0x11,
            0x5566_7788,
            TargetArchitecture::X86,
        )
        .unwrap();
        assert_eq!(
            stub,
            vec![
                0x68, 0x11, 0, 0, 0, 0x68, 0x44, 0x33, 0x22, 0x11, 0xB8, 0x88, 0x77, 0x66, 0x55,
                0xFF, 0xD0, 0xC3,
            ]
        );
    }

    #[test]
    fn generates_aligned_x64_stub() {
        let stub = build_call_stub(
            WindowHandle::new(0x1122_3344_5566_7788),
            0x11,
            0x8877_6655_4433_2211,
            TargetArchitecture::X64,
        )
        .unwrap();
        assert_eq!(&stub[..4], &[0x48, 0x83, 0xEC, 0x28]);
        assert_eq!(
            &stub[4..14],
            &[0x48, 0xB9, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11]
        );
        assert_eq!(&stub[14..19], &[0xBA, 0x11, 0, 0, 0]);
        assert_eq!(&stub[29..], &[0xFF, 0xD0, 0x48, 0x83, 0xC4, 0x28, 0xC3]);
    }
}
