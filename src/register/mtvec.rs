//! mtvec register

/// mtvec register
#[derive(Clone, Copy, Debug)]
pub struct Mtvec {
    bits: usize,
}

/// Trap mode
pub enum TrapMode {
    Direct = 0,
    Vectored = 1,
}

impl Mtvec {
    /// Returns the contents of the register as raw bits
    pub fn bits(&self) -> usize {
        self.bits
    }

    /// Returns the trap-vector base-address
    pub fn address(&self) -> usize {
        self.bits - (self.bits & 0b11)
    }

    /// Returns the trap-vector mode
    pub fn trap_mode(&self) -> TrapMode {
        let mode = self.bits & 0b11;
        match mode {
            0 => TrapMode::Direct,
            1 => TrapMode::Vectored,
            _ => unimplemented!()
        }
    }
}

/// Reads the CSR
#[inline(always)]
pub fn read() -> Mtvec {
    match () {
        #[cfg(target_arch = "riscv")]
        () => {
            let r: usize;
            unsafe {
                asm!("csrrs $0, 0x305, x0" : "=r"(r) ::: "volatile");
            }
            Mtvec { bits: r }
        }
        #[cfg(not(target_arch = "riscv"))]
        () => unimplemented!(),
    }
}

/// Writes the CSR
#[cfg_attr(not(target_arch = "riscv"), allow(unused_variables))]
#[inline(always)]
pub unsafe fn write(addr: usize, mode: TrapMode) {
    let bits = addr + mode as usize;
    match () {
        #[cfg(target_arch = "riscv")]
        () => asm!("csrrw x0, 0x305, $0" :: "r"(bits) :: "volatile"),
        #[cfg(not(target_arch = "riscv"))]
        () => unimplemented!(),
    }
}
