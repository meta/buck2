/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.aapt;

import com.google.common.base.Objects;
import org.jspecify.annotations.Nullable;

/**
 * Util class for testing {@link RDotTxtEntry} objects. Testing {@link RDotTxtEntry} objects is
 * somewhat error prone; equality and hashing on these objects is only checked against the type and
 * the name (though the comparator also considers the parent...).
 *
 * <p>Use the abstractions in this class to more easily test against the fields you care about
 * (though this is still somewhat error prone as you need to make sure you are comparing 2 {@link
 * FakeEntry} objects).
 */
public class RDotTxtEntryUtil {

  private static final String FAKE_ID = "0x00000000";

  public static RDotTxtEntry matchDefault(RDotTxtEntry entry) {
    return FakeEntry.create(entry.idType, entry.type, entry.name);
  }

  /** Return an entry that checks equality against the id + the default fields. */
  public static RDotTxtEntry matchId(RDotTxtEntry entry) {
    return FakeEntry.createWithId(entry.idType, entry.type, entry.name, entry.idValue);
  }

  /** Return an entry that checks equality against the parent + the default fields. */
  public static RDotTxtEntry matchParent(RDotTxtEntry entry) {
    return FakeEntry.createWithParent(entry.idType, entry.type, entry.name, entry.parent);
  }

  public static class FakeEntry extends RDotTxtEntry {

    public static FakeEntry create(
        RDotTxtEntry.IdType idType, RDotTxtEntry.RType type, String name) {
      return new FakeEntry(idType, type, name, FAKE_ID, null);
    }

    public static FakeEntry createWithId(
        RDotTxtEntry.IdType idType, RDotTxtEntry.RType type, String name, String idValue) {
      return new FakeEntry(idType, type, name, idValue, null);
    }

    public static FakeEntry createWithParent(
        RDotTxtEntry.IdType idType, RDotTxtEntry.RType type, String name, String parent) {
      return new FakeEntry(idType, type, name, FAKE_ID, parent);
    }

    private FakeEntry(
        IdType idType, RType type, String name, @Nullable String idValue, @Nullable String parent) {
      super(idType, type, name, idValue == null ? FAKE_ID : idValue, parent);
    }

    @Override
    public boolean equals(Object obj) {
      if (!(obj instanceof FakeEntry)) {
        throw new IllegalStateException(
            String.format(
                "Testing objects should only be compared against other testing objects. Call the"
                    + " conversion methods in the testing class. (Class: %s)",
                obj.getClass()));
      }

      FakeEntry that = (FakeEntry) obj;
      return Objects.equal(type, that.type)
          && Objects.equal(name, that.name)
          && Objects.equal(idValue, that.idValue)
          && Objects.equal(parent, that.parent);
    }

    @Override
    public int hashCode() {
      return Objects.hashCode(type, name, idValue, parent);
    }
  }
}
