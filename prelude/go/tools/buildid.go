/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package main

import (
	"crypto/sha256"
	"debug/elf"
	"fmt"
	"io"
	"os"
)

// ntGNUBuildID is the NT_GNU_BUILD_ID note type. debug/elf does not export it.
const ntGNUBuildID = 3

// outputPath returns the linker's -o value. _link_impl (link.bzl) appends the
// authoritative "-o <out>" after any user linker_flags, so return the LAST "-o"
// to avoid picking up a stray one from user-supplied flags.
func outputPath(args []string) string {
	out := ""
	for i := 0; i+1 < len(args); i++ {
		if args[i] == "-o" {
			out = args[i+1]
		}
	}
	return out
}

// setGNUBuildID fills the zeroed .note.gnu.build-id placeholder ("-B 0x00..00")
// of an internally linked Go binary with sha256 of the output. The write is in
// place and same-size, so the note stays in its runtime-visible PT_NOTE segment.
//
// Only a still-all-zero descriptor is rewritten: a non-zero one means an external
// linker owns the build-id and must not be clobbered. A non-ELF output, a missing
// note, or an externally-owned descriptor is a no-op; unreadable/corrupt output
// or a short write aborts the link, so a half-written descriptor never ships.
func setGNUBuildID(path string) error {
	descOff, descLen, err := findBuildIDNote(path)
	if err != nil || descOff == 0 {
		return err
	}

	rf, err := os.Open(path)
	if err != nil {
		return fmt.Errorf("reading linked output %q: %w", path, err)
	}
	defer rf.Close()
	fi, err := rf.Stat()
	if err != nil {
		return fmt.Errorf("stat %q for build-id: %w", path, err)
	}

	newDesc, patch, err := computeBuildID(rf, fi.Size(), descOff, descLen)
	if err != nil {
		return err
	}
	if !patch {
		return nil
	}

	f, err := os.OpenFile(path, os.O_RDWR, 0)
	if err != nil {
		return fmt.Errorf("opening %q for build-id write: %w", path, err)
	}
	if n, err := f.WriteAt(newDesc, descOff); err != nil {
		f.Close()
		return fmt.Errorf("writing build-id to %q: %w", path, err)
	} else if n != len(newDesc) {
		f.Close()
		return fmt.Errorf("writing build-id to %q: short write %d of %d bytes", path, n, len(newDesc))
	}
	if err := f.Close(); err != nil {
		return fmt.Errorf("closing %q after build-id write: %w", path, err)
	}
	return nil
}

// computeBuildID returns the descriptor bytes to write, or patch=false when the
// descriptor is not the all-zero placeholder (i.e. an external linker set it).
// It streams the output through sha256 so it never holds the whole (possibly
// multi-GB) binary in memory. The on-disk descriptor is still zero here, so
// hashing the file as-is is self-consistent and reproducible across hosts --
// the same self-reference break ld/lld use.
func computeBuildID(r io.ReaderAt, size, descOff int64, descLen int) (newDesc []byte, patch bool, err error) {
	// Bounds required to read the descriptor at all.
	if descLen <= 0 || descOff < 0 || descOff+int64(descLen) > size {
		return nil, false, fmt.Errorf("build-id descriptor out of range: off=%d len=%d file=%d", descOff, descLen, size)
	}
	// A non-zero descriptor is owned by an external linker (or a user -B): never
	// touch it, whatever its size.
	desc := make([]byte, descLen)
	if _, err := r.ReadAt(desc, descOff); err != nil {
		return nil, false, fmt.Errorf("reading build-id descriptor: %w", err)
	}
	if !allZero(desc) {
		return nil, false, nil
	}
	// We only fill our own placeholder, so it must fit the hash we write.
	if descLen > sha256.Size {
		return nil, false, fmt.Errorf("build-id placeholder too large: %d > %d bytes", descLen, sha256.Size)
	}
	h := sha256.New()
	if _, err := io.Copy(h, io.NewSectionReader(r, 0, size)); err != nil {
		return nil, false, fmt.Errorf("hashing linked output: %w", err)
	}
	return h.Sum(nil)[:descLen], true, nil
}

// findBuildIDNote returns the file offset and length of the .note.gnu.build-id
// descriptor, or (0, 0, nil) to signal "skip" (not ELF, or no build-id note).
// An I/O failure opening the output is a real error, returned not swallowed.
func findBuildIDNote(path string) (descOff int64, descLen int, err error) {
	osf, err := os.Open(path)
	if err != nil {
		return 0, 0, fmt.Errorf("opening linked output %q: %w", path, err)
	}
	defer osf.Close()

	// Skip a genuinely non-ELF output (no ELF magic) -- the Go linker silently
	// ignores the reserved -B note on non-ELF formats, so there is nothing to
	// patch. But an output that claims to be ELF yet fails to parse is a corrupt
	// or truncated linker output, which is a real error, not a skip.
	var magic [len(elf.ELFMAG)]byte
	if _, err := osf.ReadAt(magic[:], 0); err != nil {
		return 0, 0, fmt.Errorf("reading linked output %q: %w", path, err)
	}
	if string(magic[:]) != elf.ELFMAG {
		return 0, 0, nil
	}
	f, err := elf.NewFile(osf)
	if err != nil {
		return 0, 0, fmt.Errorf("parsing %q as ELF: %w", path, err)
	}

	sec := f.Section(".note.gnu.build-id")
	if sec == nil {
		return 0, 0, nil // no build-id note (e.g. an external link that emitted none)
	}
	data, err := sec.Data()
	if err != nil {
		return 0, 0, fmt.Errorf("reading .note.gnu.build-id in %q: %w", path, err)
	}

	// The section is located by name, so its contents must be a well-formed
	// NT_GNU_BUILD_ID note (name "GNU", type 3) whose descriptor lies within the
	// section. Any deviation is a corrupt linker output -- a real error, never a
	// silent skip or an out-of-section write. The || short-circuits so the name
	// slice is reached only once its bounds are known valid.
	if len(data) < 12 {
		return 0, 0, fmt.Errorf("malformed .note.gnu.build-id in %q: %d bytes", path, len(data))
	}
	namesz := f.ByteOrder.Uint32(data[0:4])
	descsz := f.ByteOrder.Uint32(data[4:8])
	typ := f.ByteOrder.Uint32(data[8:12])
	descStart := 12 + align4(int64(namesz))
	if typ != ntGNUBuildID || namesz != 4 ||
		12+int64(namesz) > int64(len(data)) || string(data[12:12+namesz]) != "GNU\x00" ||
		descStart+int64(descsz) > int64(len(data)) {
		return 0, 0, fmt.Errorf("malformed .note.gnu.build-id in %q: namesz=%d descsz=%d type=%d len=%d", path, namesz, descsz, typ, len(data))
	}
	return int64(sec.Offset) + descStart, int(descsz), nil
}

func align4(n int64) int64 {
	return (n + 3) &^ 3
}

func allZero(b []byte) bool {
	for _, x := range b {
		if x != 0 {
			return false
		}
	}
	return true
}
