// SPDX-License-Identifier: Apache-2.0 OR BSD-3-Clause

use crate::core::cpu::{self, cpuid, mptable, Vcpu};
use crate::core::devices::serial::LumperSerial;
use crate::core::epoll_context::{EpollContext, EPOLL_EVENTS_LEN};
use crate::core::kernel;
use crate::core::{Error, Result};
use event_manager::{EventManager, MutEventSubscriber};
use kvm_bindings::{kvm_userspace_memory_region, KVM_MAX_CPUID_ENTRIES};
use kvm_ioctls::{Kvm, VmFd};
use linux_loader::loader::KernelLoaderResult;
use std::io::{self, stdout, IsTerminal, Stdout};
use std::net::Ipv4Addr;
use std::os::unix::io::AsRawFd;
use std::os::unix::prelude::RawFd;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use tracing::{error, info};
use vm_allocator::{AddressAllocator, AllocPolicy};
use vm_device::bus::{MmioAddress, MmioRange};
use vm_device::device_manager::IoManager;
use vm_memory::{Address, GuestAddress, GuestMemoryBackend, GuestMemoryMmap, GuestMemoryRegion};
use vmm_sys_util::terminal::Terminal;

use super::devices::virtio::net::device::Net;
use super::devices::virtio::{self, MmioConfig};
use super::irq_allocator::IrqAllocator;
use super::slip_pty::SlipPty;

#[cfg(target_arch = "x86_64")]
pub(crate) const MMIO_GAP_END: u64 = 1 << 34;
/// Size of the MMIO gap.
#[cfg(target_arch = "x86_64")]
pub(crate) const MMIO_GAP_SIZE: u64 = 768 << 20;
/// The start of the MMIO gap (memory area reserved for MMIO devices).
#[cfg(target_arch = "x86_64")]
pub(crate) const MMIO_GAP_START: u64 = MMIO_GAP_END - MMIO_GAP_SIZE;
/// Default address allocator alignment. It needs to be a power of 2.
pub const DEFAULT_ADDRESS_ALIGNEMNT: u64 = 4;
/// Default allocation policy for address allocator.
pub const DEFAULT_ALLOC_POLICY: AllocPolicy = AllocPolicy::FirstMatch;
/// IRQ line 4 is typically used for serial port 1.
// See more IRQ assignments & info: https://tldp.org/HOWTO/Serial-HOWTO-8.html
const SERIAL_IRQ: u32 = 4;
/// Last usable IRQ ID for virtio device interrupts on x86_64.
const IRQ_MAX: u8 = 23;

type EventMgr = Arc<Mutex<EventManager<Arc<Mutex<dyn MutEventSubscriber + Send>>>>>;

pub struct VMM {
    vm_fd: Arc<VmFd>,
    kvm: Kvm,
    guest_memory: GuestMemoryMmap,
    address_allocator: Option<AddressAllocator>,
    irq_allocator: IrqAllocator,
    device_mgr: Arc<Mutex<IoManager>>,
    event_mgr: EventMgr,
    vcpus: Vec<Vcpu>,

    iface_host_addr: Ipv4Addr,
    netmask: Ipv4Addr,
    iface_guest_addr: Ipv4Addr,
    net_devices: Vec<Arc<Mutex<Net>>>,
    serial: Arc<Mutex<LumperSerial<Stdout>>>,
    slip_pty: Arc<Mutex<SlipPty>>,
    epoll: EpollContext,
    stdin_is_terminal: bool,
}

impl VMM {
    /// Create a new VMM.
    pub fn new(
        iface_host_addr: Ipv4Addr,
        netmask: Ipv4Addr,
        iface_guest_addr: Ipv4Addr,
    ) -> Result<Self> {
        // Open /dev/kvm and get a file descriptor to it.
        let kvm = Kvm::new().map_err(Error::KvmIoctl)?;

        // Create a KVM VM object.
        // KVM returns a file descriptor to the VM object.
        let vm_fd = Arc::new(kvm.create_vm().map_err(Error::KvmIoctl)?);

        let slip_pty = SlipPty::new()?;

        let epoll = EpollContext::new().map_err(Error::EpollError)?;
        let stdin_is_terminal = io::stdin().is_terminal();
        if stdin_is_terminal {
            epoll.add_stdin().map_err(Error::EpollError)?;
        }
        epoll
            .add_fd(
                slip_pty.pty_master_fd(),
                epoll::Events::EPOLLIN | epoll::Events::EPOLLET,
            )
            .map_err(Error::EpollError)?;
        epoll
            .add_fd(
                slip_pty.serial().in_buffer_empty_eventfd().as_raw_fd(),
                epoll::Events::EPOLLIN | epoll::Events::EPOLLET,
            )
            .map_err(Error::EpollError)?;

        let irq_allocator = IrqAllocator::new(SERIAL_IRQ, IRQ_MAX.into()).unwrap();
        let device_mgr = Arc::new(Mutex::new(IoManager::new()));

        let vmm = VMM {
            vm_fd,
            kvm,
            guest_memory: GuestMemoryMmap::default(),
            address_allocator: None,
            device_mgr,
            irq_allocator,
            event_mgr: Arc::new(Mutex::new(EventManager::new().unwrap())),
            vcpus: vec![],
            serial: Arc::new(Mutex::new(
                LumperSerial::new(stdout()).map_err(Error::SerialCreation)?,
            )),
            slip_pty: Arc::new(Mutex::new(slip_pty)),
            epoll,
            stdin_is_terminal,
            iface_host_addr,
            netmask,
            iface_guest_addr,
            net_devices: Vec::new(),
        };

        Ok(vmm)
    }

    fn configure_memory(&mut self, mem_size_mb: u32) -> Result<()> {
        // Convert memory size from MBytes to bytes.
        let mem_size = ((mem_size_mb as u64) << 20) as usize;

        // Create one single memory region, from zero to mem_size.
        let mem_regions = vec![(GuestAddress(0), mem_size)];

        // Allocate the guest memory from the memory region.
        let guest_memory = GuestMemoryMmap::from_ranges(&mem_regions).map_err(Error::Memory)?;

        // For each memory region in guest_memory:
        // 1. Create a KVM memory region mapping the memory region guest physical address to the host virtual address.
        // 2. Register the KVM memory region with KVM. EPTs are created then.
        for (index, region) in guest_memory.iter().enumerate() {
            let kvm_memory_region = kvm_userspace_memory_region {
                slot: index as u32,
                guest_phys_addr: region.start_addr().raw_value(),
                memory_size: region.len(),
                // It's safe to unwrap because the guest address is valid.
                userspace_addr: guest_memory.get_host_address(region.start_addr()).unwrap() as u64,
                flags: 0,
            };

            // Register the KVM memory region with KVM.
            unsafe { self.vm_fd.set_user_memory_region(kvm_memory_region) }
                .map_err(Error::KvmIoctl)?;
        }

        self.guest_memory = guest_memory;

        Ok(())
    }

    fn configure_allocators(&mut self, mem_size_mb: u32) -> Result<()> {
        // Convert memory size from MBytes to bytes.
        let mem_size = (mem_size_mb as u64) << 20;

        // Setup address allocator.
        let start_addr = MMIO_GAP_START;
        let address_allocator = AddressAllocator::new(start_addr, mem_size).unwrap();

        self.address_allocator = Some(address_allocator);

        Ok(())
    }

    fn configure_io(&mut self) -> Result<()> {
        // First, create the irqchip.
        // On `x86_64`, this _must_ be created _before_ the vCPUs.
        // It sets up the virtual IOAPIC, virtual PIC, and sets up the future vCPUs for local APIC.
        // When in doubt, look in the kernel for `KVM_CREATE_IRQCHIP`.
        // https://elixir.bootlin.com/linux/latest/source/arch/x86/kvm/x86.c
        self.vm_fd.create_irq_chip().map_err(Error::KvmIoctl)?;

        self.vm_fd
            .register_irqfd(
                &self
                    .serial
                    .lock()
                    .unwrap()
                    .eventfd()
                    .map_err(Error::IrqRegister)?,
                4,
            )
            .map_err(Error::KvmIoctl)?;

        self.vm_fd
            .register_irqfd(
                &self
                    .slip_pty
                    .lock()
                    .unwrap()
                    .serial()
                    .eventfd()
                    .map_err(Error::IrqRegister)?,
                3,
            )
            .map_err(Error::KvmIoctl)?;

        for net in self.net_devices.iter() {
            let net_cfg = &net.lock().unwrap().config;

            self.vm_fd
                .register_irqfd(&net_cfg.irqfd, net_cfg.mmio.gsi)
                .map_err(Error::KvmIoctl)?;
        }

        Ok(())
    }

    fn configure_vcpus(&mut self, num_vcpus: u8, kernel_load: KernelLoaderResult) -> Result<()> {
        mptable::setup_mptable(&self.guest_memory, num_vcpus)
            .map_err(|e| Error::Vcpu(cpu::Error::Mptable(e)))?;

        let base_cpuid = self
            .kvm
            .get_supported_cpuid(KVM_MAX_CPUID_ENTRIES)
            .map_err(Error::KvmIoctl)?;

        for index in 0..num_vcpus {
            let vcpu = Vcpu::new(
                &self.vm_fd,
                index.into(),
                self.device_mgr.clone(),
                Arc::clone(&self.serial),
                Arc::clone(&self.slip_pty),
            )
            .map_err(Error::Vcpu)?;

            // Set CPUID.
            let mut vcpu_cpuid = base_cpuid.clone();
            cpuid::filter_cpuid(
                &self.kvm,
                index as usize,
                num_vcpus as usize,
                &mut vcpu_cpuid,
            );
            vcpu.configure_cpuid(&vcpu_cpuid).map_err(Error::Vcpu)?;

            // Configure MSRs (model specific registers).
            vcpu.configure_msrs().map_err(Error::Vcpu)?;

            // Configure regs, sregs and fpu.
            vcpu.configure_regs(kernel_load.kernel_load)
                .map_err(Error::Vcpu)?;
            vcpu.configure_sregs(&self.guest_memory)
                .map_err(Error::Vcpu)?;
            vcpu.configure_fpu().map_err(Error::Vcpu)?;

            // Configure LAPICs.
            vcpu.configure_lapic().map_err(Error::Vcpu)?;

            self.vcpus.push(vcpu);
        }

        Ok(())
    }

    /// Run all virtual CPUs.
    pub fn run(&mut self) -> Result<()> {
        for mut vcpu in self.vcpus.drain(..) {
            info!(vcpu_index = vcpu.index, "Starting vCPU");
            let _ = thread::Builder::new().spawn(move || loop {
                vcpu.run();
            });
        }

        let stdin = io::stdin();
        let stdin_lock = stdin.lock();
        if self.stdin_is_terminal {
            stdin_lock
                .set_raw_mode()
                .map_err(Error::TerminalConfigure)?;
        }
        let mut events = [epoll::Event::new(epoll::Events::empty(), 0); EPOLL_EVENTS_LEN];
        let epoll_fd = self.epoll.as_raw_fd();

        let event_mgr = self.event_mgr.clone();
        let _ = thread::Builder::new().spawn(move || loop {
            match event_mgr.lock().unwrap().run() {
                Ok(_) => (),
                Err(e) => eprintln!("Failed to handle events: {:?}", e),
            }
        });

        // Let's start the STDIN polling thread.
        loop {
            let num_events =
                epoll::wait(epoll_fd, -1, &mut events[..]).map_err(Error::EpollError)?;

            for event in events.iter().take(num_events) {
                let event_evts = epoll::Events::from_bits_truncate(event.events);
                let event_data = event.data as RawFd;

                if self.stdin_is_terminal && event_data == libc::STDIN_FILENO {
                    let mut out = [0u8; 64];

                    let count = stdin_lock.read_raw(&mut out).map_err(Error::StdinRead)?;

                    self.serial
                        .lock()
                        .unwrap()
                        .serial
                        .enqueue_raw_bytes(&out[..count])
                        .map_err(Error::StdinWrite)?;
                } else if event_evts.intersects(epoll::Events::EPOLLIN)
                    && event_data == self.slip_pty.lock().unwrap().pty_master_fd()
                {
                    self.slip_pty.lock().unwrap().handle_master_rx()?;
                } else if event_data
                    == self
                        .slip_pty
                        .lock()
                        .unwrap()
                        .serial()
                        .in_buffer_empty_eventfd()
                        .as_raw_fd()
                {
                    self.slip_pty
                        .lock()
                        .unwrap()
                        .serial_mut()
                        .flush_in_buffer()
                        .map_err(Error::PtyRx)?;
                }
            }
        }
    }

    /// Configure the VMM:
    /// * `num_vcpus` Number of virtual CPUs
    /// * `mem_size_mb` Memory size (in MB)
    /// * `kernel_path` Path to a Linux kernel
    /// * `initramfs_path` Path to an initramfs
    pub async fn configure(
        &mut self,
        num_vcpus: u8,
        mem_size_mb: u32,
        kernel_path: PathBuf,
        initramfs_path: &Option<PathBuf>,
    ) -> Result<()> {
        self.configure_with_boot_parameters(
            num_vcpus,
            mem_size_mb,
            kernel_path,
            initramfs_path,
            Vec::new(),
        )
        .await
    }

    /// Configures a VM with Cloudlet-owned kernel parameters in addition to
    /// device-generated parameters. The privileged VMM uses this to pass a
    /// per-boot agent credential without putting it in a workload environment.
    pub async fn configure_with_boot_parameters(
        &mut self,
        num_vcpus: u8,
        mem_size_mb: u32,
        kernel_path: PathBuf,
        initramfs_path: &Option<PathBuf>,
        mut cmdline_extra_parameters: Vec<String>,
    ) -> Result<()> {
        self.configure_memory(mem_size_mb)?;
        self.configure_allocators(mem_size_mb)?;
        self.configure_net_device(&mut cmdline_extra_parameters)
            .await?;

        let kernel_load = kernel::kernel_setup(
            &self.guest_memory,
            kernel_path,
            initramfs_path.clone(),
            &mut cmdline_extra_parameters,
        )?;
        self.configure_io()?;
        self.configure_vcpus(num_vcpus, kernel_load)?;

        Ok(())
    }

    pub async fn configure_net_device(
        &mut self,
        cmdline_extra_parameters: &mut Vec<String>,
    ) -> Result<()> {
        let mem = Arc::new(self.guest_memory.clone());
        let range = if let Some(allocator) = &self.address_allocator {
            allocator
                .to_owned()
                .allocate(0x1000, DEFAULT_ADDRESS_ALIGNEMNT, DEFAULT_ALLOC_POLICY)
                .map_err(Error::Allocate)?
        } else {
            // Handle the case where self.address_allocator is None
            panic!("Address allocator is not initialized");
        };
        let mmio_range = MmioRange::new(MmioAddress(range.start()), range.len())
            .map_err(|_| Error::MmioRange)?;
        let irq = self.irq_allocator.next_irq().map_err(Error::IrqAllocator)?;
        let mmio_cfg = MmioConfig {
            range: mmio_range,
            gsi: irq,
        };

        let remote_endpoint = { self.event_mgr.lock().unwrap().remote_endpoint() };

        let net = Net::new(
            mem,
            self.device_mgr.clone(),
            mmio_cfg,
            self.iface_host_addr,
            self.netmask,
            self.iface_guest_addr,
            irq,
            remote_endpoint,
            self.vm_fd.clone(),
            cmdline_extra_parameters,
        )
        .await
        .map_err(|err| {
            error!("could not configure Net device: {:?}", err);
            // not good, the error should be propagated
            // these error enums should be re-written...
            Error::Virtio(virtio::Error::Net)
        })?;

        self.net_devices.push(net);

        Ok(())
    }
}
