#![cfg(target_arch = "x86_64")]
extern crate libc;

extern crate linux_loader;
extern crate vm_memory;
extern crate vm_superio;

use linux_loader::loader;
use std::io;

use self::devices::virtio::{self, net::tuntap::open_tap};

mod cpu;
mod devices;
mod epoll_context;
mod irq_allocator;
mod kernel;
mod slip_pty;
pub mod vmm;

#[derive(Debug)]
/// VMM errors.
pub enum Error {
    /// Failed to write boot parameters to guest memory.
    BootConfigure(linux_loader::configurator::Error),
    /// Error configuring the kernel command line.
    Cmdline(linux_loader::cmdline::Error),
    /// Failed to load kernel.
    KernelLoad(loader::Error),
    /// Failed to load the initramfs.
    InitramfsLoad,
    /// Invalid E820 configuration.
    E820Configuration,
    /// Highmem start address is past the guest memory end.
    HimemStartPastMemEnd,
    /// I/O error.
    IO(io::Error),
    /// Error issuing an ioctl to KVM.
    KvmIoctl(kvm_ioctls::Error),
    /// vCPU errors.
    Vcpu(cpu::Error),
    /// Memory error.
    Memory(vm_memory::mmap::FromRangesError),
    /// Serial creation error
    SerialCreation(devices::Error),
    /// PTY creation error
    PtyCreation,
    /// PTY set up error
    PtySetup,
    /// IRQ registration error
    IrqRegister(devices::Error),
    /// epoll creation error
    EpollError(io::Error),
    /// STDIN read error
    StdinRead(vmm_sys_util::errno::Error),
    /// STDIN write error
    StdinWrite(vm_superio::serial::Error<io::Error>),
    /// PTY read error
    PtyRead(io::Error),
    /// PTY serial write error
    PtySerialWrite(vm_superio::serial::Error<io::Error>),
    /// Terminal configuration error
    TerminalConfigure(vmm_sys_util::errno::Error),
    // Tap open error
    OpenTap(open_tap::Error),
    // PTY write error
    PtyRx(devices::Error),
    // Address allocator
    Allocate(vm_allocator::Error),
    // Address allocator
    IrqAllocator(irq_allocator::Error),
    // MmioRange
    MmioRange,
    // Virtio net
    Virtio(virtio::Error),
}

/// Dedicated [`Result`](https://doc.rust-lang.org/std/result/) type.
pub type Result<T> = std::result::Result<T, Error>;
