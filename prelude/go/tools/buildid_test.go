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
	"bytes"
	"crypto/sha256"
	"debug/elf"
	"encoding/binary"
	"os"
	"path/filepath"
	"testing"
)

// newFile returns a synthetic "binary" with a descLen-byte descriptor at descOff.
func newFile(size int, descOff int64, descLen int, descFill byte) []byte {
	buf := make([]byte, size)
	for i := range buf {
		buf[i] = byte(i)
	}
	for i := 0; i < descLen; i++ {
		buf[descOff+int64(i)] = descFill
	}
	return buf
}

func TestComputeBuildIDPatchesZeroPlaceholder(t *testing.T) {
	const size, descOff, descLen = 256, 64, 20
	buf := newFile(size, descOff, descLen, 0)

	desc, patch, err := computeBuildID(bytes.NewReader(buf), int64(len(buf)), descOff, descLen)
	if err != nil || !patch {
		t.Fatalf("computeBuildID: patch=%v err=%v, want patch=true nil", patch, err)
	}
	if len(desc) != descLen {
		t.Fatalf("descriptor len = %d, want %d", len(desc), descLen)
	}
	// Self-consistency: the hash must be over the file with the descriptor zeroed.
	want := sha256.Sum256(buf)
	if !bytes.Equal(desc, want[:descLen]) {
		t.Fatalf("descriptor = %x, want %x", desc, want[:descLen])
	}
}

func TestComputeBuildIDSensitiveToContent(t *testing.T) {
	base := newFile(256, 64, 20, 0)
	a, _, _ := computeBuildID(bytes.NewReader(base), int64(len(base)), 64, 20)
	// Flip a byte outside the descriptor region.
	other := newFile(256, 64, 20, 0)
	other[10] ^= 0xff
	b, _, _ := computeBuildID(bytes.NewReader(other), int64(len(other)), 64, 20)
	if bytes.Equal(a, b) {
		t.Fatalf("build-id unchanged after content change: %x", a)
	}
}

func TestComputeBuildIDSkipsNonZeroDescriptor(t *testing.T) {
	buf := newFile(256, 64, 20, 0xaa) // external linker already set it
	desc, patch, err := computeBuildID(bytes.NewReader(buf), int64(len(buf)), 64, 20)
	if err != nil {
		t.Fatalf("unexpected err: %v", err)
	}
	if patch || desc != nil {
		t.Fatalf("expected skip, got patch=%v desc=%x", patch, desc)
	}
}

func TestComputeBuildIDRejectsOutOfRange(t *testing.T) {
	zeros := make([]byte, 64)
	for _, tc := range []struct {
		name    string
		buf     []byte
		descOff int64
		descLen int
	}{
		{"len non-positive", zeros, 0, 0},
		{"off negative", zeros, -1, 20},
		{"past end", zeros, 60, 20},
		// An all-zero placeholder we would fill must fit the hash we write.
		{"placeholder too large", zeros, 0, sha256.Size + 1},
	} {
		if _, _, err := computeBuildID(bytes.NewReader(tc.buf), int64(len(tc.buf)), tc.descOff, tc.descLen); err == nil {
			t.Errorf("%s: expected error, got nil", tc.name)
		}
	}
}

// A non-zero descriptor is externally owned and must be skipped, not rejected,
// even when it is larger than our hash (guards the check-ordering fix).
func TestComputeBuildIDSkipsLargeNonZeroDescriptor(t *testing.T) {
	buf := newFile(64, 0, sha256.Size+1, 0xaa)
	desc, patch, err := computeBuildID(bytes.NewReader(buf), int64(len(buf)), 0, sha256.Size+1)
	if err != nil || patch || desc != nil {
		t.Fatalf("large non-zero descriptor: desc=%x patch=%v err=%v, want skip", desc, patch, err)
	}
}

func TestOutputPath(t *testing.T) {
	for _, tc := range []struct {
		args []string
		want string
	}{
		{[]string{"-buildmode=exe", "-o", "out/bin", "main.o"}, "out/bin"},
		{[]string{"-buildmode=exe", "main.o"}, ""},
		{[]string{"main.o", "-o"}, ""}, // dangling -o must not panic
		// A stray -o in user linker_flags must not win over _link_impl's trailing -o.
		{[]string{"-o", "stray", "-buildmode=exe", "-o", "out/bin", "main.o"}, "out/bin"},
	} {
		if got := outputPath(tc.args); got != tc.want {
			t.Errorf("outputPath(%v) = %q, want %q", tc.args, got, tc.want)
		}
	}
}

func TestAlign4(t *testing.T) {
	for in, want := range map[int64]int64{0: 0, 1: 4, 4: 4, 5: 8} {
		if got := align4(in); got != want {
			t.Errorf("align4(%d) = %d, want %d", in, got, want)
		}
	}
}

// craftELFWithZeroNote builds a minimal little-endian ELF64 with a
// .note.gnu.build-id section whose descLen-byte descriptor is all zero (the
// placeholder Go's `-B 0x00..00` writes). The note contents sit at file offset
// 64, so the descriptor lands at 64 + 12 (note header) + 4 (aligned "GNU\0") = 80.
func craftELFWithZeroNote(descLen int) []byte {
	const ehdrSize, shdrSize, noteOff = 64, 64, 64

	note := make([]byte, 12+4+descLen)
	binary.LittleEndian.PutUint32(note[0:], 4)               // namesz
	binary.LittleEndian.PutUint32(note[4:], uint32(descLen)) // descsz
	binary.LittleEndian.PutUint32(note[8:], ntGNUBuildID)    // type
	copy(note[12:], "GNU\x00")                               // name; descriptor left zero

	shstr := []byte("\x00.note.gnu.build-id\x00.shstrtab\x00")
	shstrOff := noteOff + len(note)
	shoff := shstrOff + len(shstr)
	buf := make([]byte, shoff+3*shdrSize)

	copy(buf, []byte{0x7f, 'E', 'L', 'F', 2, 1, 1, 0}) // ident: ELF64, LE
	binary.LittleEndian.PutUint16(buf[16:], 2)         // ET_EXEC
	binary.LittleEndian.PutUint16(buf[18:], 62)        // EM_X86_64
	binary.LittleEndian.PutUint32(buf[20:], 1)         // version
	binary.LittleEndian.PutUint64(buf[40:], uint64(shoff))
	binary.LittleEndian.PutUint16(buf[52:], ehdrSize)
	binary.LittleEndian.PutUint16(buf[58:], shdrSize)
	binary.LittleEndian.PutUint16(buf[60:], 3) // shnum
	binary.LittleEndian.PutUint16(buf[62:], 2) // shstrndx
	copy(buf[noteOff:], note)
	copy(buf[shstrOff:], shstr)

	sh1 := shoff + shdrSize                                        // .note.gnu.build-id
	binary.LittleEndian.PutUint32(buf[sh1:], 1)                    // name offset
	binary.LittleEndian.PutUint32(buf[sh1+4:], 7)                  // SHT_NOTE
	binary.LittleEndian.PutUint64(buf[sh1+8:], 2)                  // SHF_ALLOC
	binary.LittleEndian.PutUint64(buf[sh1+24:], uint64(noteOff))   // offset
	binary.LittleEndian.PutUint64(buf[sh1+32:], uint64(len(note))) // size
	binary.LittleEndian.PutUint64(buf[sh1+48:], 4)                 // addralign

	sh2 := shoff + 2*shdrSize                                       // .shstrtab
	binary.LittleEndian.PutUint32(buf[sh2:], 20)                    // name offset
	binary.LittleEndian.PutUint32(buf[sh2+4:], 3)                   // SHT_STRTAB
	binary.LittleEndian.PutUint64(buf[sh2+24:], uint64(shstrOff))   // offset
	binary.LittleEndian.PutUint64(buf[sh2+32:], uint64(len(shstr))) // size
	binary.LittleEndian.PutUint64(buf[sh2+48:], 1)                  // addralign
	return buf
}

func TestFindAndSetBuildIDNote(t *testing.T) {
	const descLen = 20
	path := filepath.Join(t.TempDir(), "bin")
	orig := craftELFWithZeroNote(descLen)
	if err := os.WriteFile(path, orig, 0o644); err != nil {
		t.Fatal(err)
	}

	descOff, gotLen, err := findBuildIDNote(path)
	if err != nil {
		t.Fatalf("findBuildIDNote: %v", err)
	}
	if descOff != 80 || gotLen != descLen {
		t.Fatalf("findBuildIDNote = (%d,%d), want (80,%d)", descOff, gotLen, descLen)
	}

	if err := setGNUBuildID(path); err != nil {
		t.Fatalf("setGNUBuildID: %v", err)
	}
	patched, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if len(patched) != len(orig) {
		t.Fatalf("file size changed: %d -> %d (write not in place)", len(orig), len(patched))
	}
	got := patched[descOff : descOff+int64(descLen)]
	if allZero(got) {
		t.Fatal("descriptor still all-zero after patch")
	}
	zeroed := append([]byte(nil), patched...)
	for i := 0; i < descLen; i++ {
		zeroed[descOff+int64(i)] = 0
	}
	want := sha256.Sum256(zeroed)
	if !bytes.Equal(got, want[:descLen]) {
		t.Fatalf("build-id = %x, want sha256(output,desc zeroed)[:20] = %x", got, want[:descLen])
	}

	// Idempotent: the now non-zero descriptor is treated as externally owned -> skip.
	if err := setGNUBuildID(path); err != nil {
		t.Fatalf("second setGNUBuildID: %v", err)
	}
	again, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(patched, again) {
		t.Fatal("second setGNUBuildID modified an already-set (non-zero) descriptor")
	}
}

// TestFindBuildIDNoteNonELF confirms a non-ELF output (e.g. a wasm target that
// passed the gate) is a clean skip, not an error.
func TestFindBuildIDNoteNonELF(t *testing.T) {
	path := filepath.Join(t.TempDir(), "not-elf")
	if err := os.WriteFile(path, []byte("#!/bin/sh\necho hi\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	descOff, descLen, err := findBuildIDNote(path)
	if err != nil || descOff != 0 || descLen != 0 {
		t.Fatalf("findBuildIDNote(non-ELF) = (%d,%d,%v), want (0,0,nil)", descOff, descLen, err)
	}
}

// TestFindBuildIDNoteMissingFile confirms a missing/unreadable output is a real
// error, not swallowed.
func TestFindBuildIDNoteMissingFile(t *testing.T) {
	if _, _, err := findBuildIDNote(filepath.Join(t.TempDir(), "does-not-exist")); err == nil {
		t.Fatal("findBuildIDNote(missing) = nil error, want non-nil (must not swallow I/O errors)")
	}
}

// TestFindBuildIDNoteCorruptELF confirms an output with ELF magic that fails to
// parse is a real error, not silently skipped like a genuine non-ELF file.
func TestFindBuildIDNoteCorruptELF(t *testing.T) {
	path := filepath.Join(t.TempDir(), "corrupt")
	if err := os.WriteFile(path, append([]byte(elf.ELFMAG), 0, 0, 0, 0), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, _, err := findBuildIDNote(path); err == nil {
		t.Fatal("findBuildIDNote(corrupt ELF) = nil error, want non-nil (must not swallow corrupt output)")
	}
}

// TestFindBuildIDNoteDescriptorOverrun confirms a note whose descsz claims more
// than the section holds is a real error, not an out-of-section read/write.
func TestFindBuildIDNoteDescriptorOverrun(t *testing.T) {
	buf := craftELFWithZeroNote(20)
	// Corrupt descsz (2nd u32 of the note header, at file offset 64+4) to overrun.
	binary.LittleEndian.PutUint32(buf[64+4:], 1<<20)
	path := filepath.Join(t.TempDir(), "overrun")
	if err := os.WriteFile(path, buf, 0o644); err != nil {
		t.Fatal(err)
	}
	if _, _, err := findBuildIDNote(path); err == nil {
		t.Fatal("findBuildIDNote(oversized descsz) = nil error, want non-nil (descriptor overruns section)")
	}
}

// TestFindBuildIDNoteTruncatedNote confirms a note section too short to even hold
// its declared name is a real error, not a silent skip. craftELFWithZeroNote(20)
// layout: ehdr[0:64], note[64:100], shstrtab[100:130], shoff=130; shdr[1] starts
// at 194 and its sh_size field is at +32 = 226.
func TestFindBuildIDNoteTruncatedNote(t *testing.T) {
	buf := craftELFWithZeroNote(20)
	binary.LittleEndian.PutUint64(buf[226:], 14) // shrink note section below the 16-byte name end
	path := filepath.Join(t.TempDir(), "trunc")
	if err := os.WriteFile(path, buf, 0o644); err != nil {
		t.Fatal(err)
	}
	if _, _, err := findBuildIDNote(path); err == nil {
		t.Fatal("findBuildIDNote(truncated note) = nil error, want non-nil (corrupt note, not skip)")
	}
}
