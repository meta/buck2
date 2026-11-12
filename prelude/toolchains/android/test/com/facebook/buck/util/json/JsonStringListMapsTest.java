/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util.json;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertThrows;

import com.google.common.collect.ImmutableSortedMap;
import com.google.common.collect.ImmutableSortedSet;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.Collection;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.TreeMap;
import java.util.TreeSet;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

public class JsonStringListMapsTest {
  @Rule public TemporaryFolder temporaryFolder = new TemporaryFolder();

  @Test
  public void writesTheSameBytesAsObjectMappers() throws Exception {
    Map<String, Set<String>> strings = new TreeMap<>();
    strings.put("p/A.class", new TreeSet<>(List.of("B.kt", "A.kt")));
    strings.put("p/Empty.class", new TreeSet<>());
    strings.put(
        "quote\" backslash\\ slash/ tab\t newline\n control\u0001 \u007f",
        new TreeSet<>(List.of("é ✓ 𝄞 \u2028", "\u001f\b\f\r")));
    Map<Path, Set<Path>> paths =
        ImmutableSortedMap.<Path, Set<Path>>of(
            Paths.get("buck-out/v2/a.jar"),
            ImmutableSortedSet.of(Paths.get("p/Y.class"), Paths.get("p/X.class")),
            Paths.get("with space/b.jar"),
            ImmutableSortedSet.of());
    List<Map<?, ? extends Collection<?>>> maps = List.of(strings, paths, Map.of());
    for (Map<?, ? extends Collection<?>> map : maps) {
      Path expected = temporaryFolder.newFile().toPath();
      Path actual = temporaryFolder.newFile().toPath();
      ObjectMappers.WRITER.writeValue(expected.toFile(), map);
      JsonStringListMaps.write(actual, map);
      assertArrayEquals(Files.readAllBytes(expected), Files.readAllBytes(actual));
    }
  }

  @Test
  public void readsWhatItWrites() throws Exception {
    Map<String, Set<String>> map = new TreeMap<>();
    map.put("a\"b", new TreeSet<>(List.of("x", "y\n")));
    map.put("empty", new TreeSet<>());
    Path file = temporaryFolder.newFile().toPath();
    JsonStringListMaps.write(file, map);
    assertEquals(
        Map.of("a\"b", List.of("x", "y\n"), "empty", List.of()), JsonStringListMaps.read(file));
  }

  @Test
  public void readsObjectsOfStringArraysInFileOrder() throws Exception {
    Path input = temporaryFolder.newFile().toPath();
    Files.writeString(input, "{\"b\":[\"2\",\"1\"],\"a\":[],\"c\":[\"\\u00e9\\n\",3,true]}");
    Map<String, List<String>> map = JsonStringListMaps.read(input);
    assertEquals(List.of("b", "a", "c"), List.copyOf(map.keySet()));
    assertEquals(
        Map.of("b", List.of("2", "1"), "a", List.of(), "c", List.of("é\n", "3", "true")), map);
  }

  @Test
  public void rejectsOtherShapes() throws Exception {
    Path input = temporaryFolder.newFile().toPath();
    for (String json :
        List.of(
            "",
            "null",
            "[]",
            "\"a\"",
            "{\"a\":null}",
            "{\"a\":\"b\"}",
            "{\"a\":[null]}",
            "{\"a\":[[\"b\"]]}",
            "{\"a\":[{}]}",
            "{\"a\":[],\"a\":[]}",
            "{\"a\":[\"b\"]")) {
      Files.writeString(input, json);
      assertThrows(json, IOException.class, () -> JsonStringListMaps.read(input));
    }
  }
}
