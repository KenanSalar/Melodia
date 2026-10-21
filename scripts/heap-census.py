#!/usr/bin/env python3
"""Read-only census of a live glibc process heap.

Walks glibc's main arena, each 64 MiB-aligned secondary arena and every mmapped chunk through
`/proc/<pid>/mem`, reads residency from `/proc/<pid>/pagemap`, and identifies Slint pixel buffers by
their `SharedVector` header (refcount, `len == cap`, chunk = 24 + bpp x len). Nothing is stopped or
injected, but reading another process's memory needs `kernel.yama.ptrace_scope` at 0 (or the same
uid under a scope that allows the attach).

`--per-track N` also lists mmapped chunks as bytes per track and every main-arena size class holding
at least 0.8 x N live chunks. A column array shows up as an mmapped chunk whose bytes per track is a
whole number; a per-entry object as a size class at about 1.00 per track.

    python3 -I scripts/heap-census.py "$(pgrep -x Melodia)" --per-track 51012
"""
import argparse
import collections
import struct

PAGE = 4096
HEADER = 24  # slint SharedVectorHeader: refcount, len, capacity
MIB = 1048576


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("pid", type=int)
    parser.add_argument("--per-track", type=int, metavar="N", help="track count to divide by")
    args = parser.parse_args()
    census = Census(args.pid)
    census.print_split()
    census.walk()
    census.print_summary()
    if args.per_track:
        census.print_per_track(args.per_track)


class Census:
    def __init__(self, pid):
        self.pid = pid
        self.mem = open(f"/proc/{pid}/mem", "rb", buffering=0)
        self.pagemap = open(f"/proc/{pid}/pagemap", "rb", buffering=0)
        self.maps = []
        for line in open(f"/proc/{pid}/maps"):
            parts = line.split()
            lo, hi = (int(x, 16) for x in parts[0].split("-"))
            self.maps.append((lo, hi, parts[1], parts[5] if len(parts) > 5 else ""))
        self.live = 0
        self.free_fragments = 0
        self.free_trimmable = 0
        self.pixels = collections.defaultdict(lambda: [0, 0])
        self.main_sizes = collections.Counter()
        self.mmapped = []

    def read(self, addr, n):
        self.mem.seek(addr)
        return self.mem.read(n)

    def u64(self, addr):
        return struct.unpack("<Q", self.read(addr, 8))[0]

    def present_pages(self, lo, hi):
        first, last = -(-lo // PAGE), hi // PAGE
        if last <= first:
            return 0
        self.pagemap.seek(first * 8)
        raw = self.pagemap.read((last - first) * 8)
        return sum(1 for (e,) in struct.iter_unpack("<Q", raw) if e >> 63 & 1)

    def print_split(self):
        heap = anon = libs = pss = 0
        name = ""
        for line in open(f"/proc/{self.pid}/smaps"):
            fields = line.split()
            if "-" in fields[0] and len(fields) >= 5 and not fields[0].endswith(":"):
                name = fields[5] if len(fields) >= 6 else "[anon]"
            elif fields[0] == "Pss:":
                pss += int(fields[1])
            elif fields[0] == "Anonymous:":
                kib = int(fields[1])
                if name == "[heap]":
                    heap += kib
                elif name == "[anon]":
                    anon += kib
                else:
                    libs += kib
        status = {}
        for line in open(f"/proc/{self.pid}/status"):
            key, _, value = line.partition(":")
            status[key] = value.strip()
        print(f"VmRSS {status['VmRSS']}  RssAnon {status['RssAnon']}  RssFile {status['RssFile']}  "
              f"RssShmem {status['RssShmem']}  VmHWM {status['VmHWM']}")
        print(f"PSS {pss / 1024:.1f} MiB; Anonymous {(heap + anon + libs) / 1024:.1f} = [heap] "
              f"{heap / 1024:.1f} + other anon {anon / 1024:.1f} + inside library mappings {libs / 1024:.1f} MiB")

    def walk_chain(self, start, end):
        p = start
        while p + 16 <= end:
            size = self.u64(p + 8) & ~7
            if size < 32 or p + size > end:
                return
            nxt = p + size
            if nxt + 16 > end:
                yield p, size, False
                return
            yield p, size, bool(self.u64(nxt + 8) & 1)
            p = nxt

    def pixel_tag(self, addr, usable, slack):
        if usable < HEADER + 3 * 64:
            return None
        refcount, length, capacity = struct.unpack("<qQQ", self.read(addr + 16, 24))
        if not (0 < refcount < 4096 and length == capacity and length >= 64):
            return None
        for bpp, tag in ((3, "rgb8"), (4, "rgba8")):
            need = HEADER + bpp * length
            if need <= usable < need + slack:
                side = int(round(length ** 0.5))
                return f"{tag} {side}x{side}" if side * side == length else f"{tag} {length}px"
        return None

    def record_live(self, addr, usable, resident, slack):
        self.live += resident
        tag = self.pixel_tag(addr, usable, slack)
        if tag:
            self.pixels[tag][0] += 1
            self.pixels[tag][1] += resident

    def walk(self):
        heap = next(m for m in self.maps if m[3] == "[heap]")
        arenas = [(heap[0], heap[1], "main")]
        for lo, hi, perm, name in self.maps:
            if name == "" and perm == "rw-p" and lo % (64 << 20) == 0:
                used = self.u64(lo + 16)  # heap_info.size
                if 0 < used <= hi - lo:
                    arenas.append((lo, lo + used, "arena"))
        for lo, hi, kind in arenas:
            start = lo if kind == "main" else self.first_chunk(lo, hi)
            if start is None:
                print(f"no chunk chain found in arena at {lo:#x}")
                continue
            for addr, size, used in self.walk_chain(start, hi):
                if used:
                    if kind == "main":
                        self.main_sizes[size] += 1
                    resident = size if size < 64 * 1024 else self.present_pages(addr, addr + size) * PAGE
                    self.record_live(addr, size - 8, resident, 32)
                elif size < 2 * PAGE:
                    self.free_fragments += size
                else:
                    self.free_trimmable += self.present_pages(addr + 48, addr + size) * PAGE
        arena_starts = {a[0] for a in arenas}
        for lo, hi, perm, name in self.maps:
            if name or perm != "rw-p" or lo in arena_starts:
                continue
            p = lo
            while p + 16 <= hi:
                try:
                    raw = self.u64(p + 8)
                except OSError:
                    break  # driver-backed VMA, not malloc's
                size = raw & ~7
                span = -(-size // PAGE) * PAGE
                if not raw & 2 or size == 0 or p + span > hi:
                    break
                resident = self.present_pages(p, p + span) * PAGE
                self.mmapped.append((size, resident))
                self.record_live(p, size - 16, resident, PAGE + 32)
                p += span

    def first_chunk(self, lo, hi):
        """A secondary arena's first chunk follows heap_info and malloc_state; find the offset
        whose chain ends exactly on the top chunk."""
        for off in range(0x30, 0x1400, 16):
            last = None
            for last in self.walk_chain(lo + off, hi):
                pass
            if last and last[0] + last[1] == hi and not last[2]:
                return lo + off
        return None

    def print_summary(self):
        print(f"live heap (resident): {self.live / MIB:.2f} MiB, {len(self.mmapped)} mmapped chunks")
        print(f"free but resident: {self.free_trimmable / MIB:.2f} MiB in whole pages "
              f"(malloc_trim can return), ~{self.free_fragments / MIB:.2f} MiB in sub-page fragments")
        total = 0
        for tag, (count, resident) in sorted(self.pixels.items(), key=lambda kv: -kv[1][1]):
            total += resident
            print(f"  pixels {tag:16s} x{count:4d}  {resident / MIB:6.2f} MiB")
        print(f"  pixels total {total / MIB:.2f} MiB")

    def print_per_track(self, tracks):
        print(f"\nmmapped chunks, largest first (bytes per track at {tracks}):")
        for size, resident in sorted(self.mmapped, reverse=True)[:16]:
            print(f"  {size:10d} B  resident {resident / MIB:6.2f} MiB  {size / tracks:6.1f} B per track")
        print(f"main-arena size classes holding >= {int(tracks * 0.8)} live chunks:")
        for size, count in sorted(self.main_sizes.items()):
            if count >= tracks * 0.8:
                print(f"  {size:5d} B x{count:6d} = {size * count / MIB:6.2f} MiB  ({count / tracks:.2f} per track)")


if __name__ == "__main__":
    main()
