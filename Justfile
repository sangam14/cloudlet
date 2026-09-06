set shell := ["/bin/bash", "-uc"]

# Build-time Node only; the result serves the console without Node or a WebView.
build-console:
  npm --prefix frontend ci
  npm --prefix frontend run build
  cargo build --release -p cloudlet

console:
  ./target/release/cloudlet dashboard

test-console:
  npm --prefix frontend test
  npm --prefix frontend run build
  cargo test --workspace

setup:
  #!/bin/bash
  set -e
  just build-kernel
  just build-rootfs

run:
  #!/bin/bash
  CARGO_PATH=$(which cargo)
  sudo -E capsh --keep=1 --user=$USER --inh=cap_net_admin --addamb=cap_net_admin -- -c \
    'RUST_BACKTRACE=1 '$CARGO_PATH' run --bin vmm -- cli --memory 512 --cpus 1 \
    --kernel tools/kernel/linux-cloud-hypervisor/arch/x86/boot/compressed/vmlinux.bin \
    --iface-host-addr 172.29.0.1 --netmask 255.255.0.0 --iface-guest-addr 172.29.0.2 \
    --initramfs=../virt-do/initramfs.img'

build-kernel:
  #!/bin/bash
  pushd tools/kernel
  ./mkkernel.sh
  popd

build-agent args = "":
  #!/bin/bash
  docker run --rm \
    -v cargo-cache:/root/.cargo \
    -v $PWD:/volume \
    -w /volume \
    -t clux/muslrust \
    cargo build --release --bin agent {{args}}
  
build-musl-agent args = "":
  #!/bin/bash
  rustup target add x86_64-unknown-linux-musl
  cargo build --release --bin agent --target=x86_64-unknown-linux-musl

build-rootfs mode = "dev":
  #!/bin/bash
  set -e
  if [ "{{mode}}" = "dev" ]; then
    echo "Building rootfs in debug mode"
    just build-agent "--features debug-agent"
  else
    echo "Building rootfs in release mode"
    just build-agent
  fi
  pushd tools/rootfs
  ./mkrootfs.sh
  popd

configure-slip pts_num:
  #!/bin/bash
  if [ "$(id -u)" -ne 0 ]; then
    echo "Please run as root"
    exit 1
  fi
  slattach -L /dev/pts/{{pts_num}} &
  sleep 5
  ip a add 172.30.0.10/16 dev sl0
  ip l set sl0 up
