/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.build_config;

import com.facebook.buck.android.build_config.BuildConfigFields.Field;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.base.Joiner;
import com.google.common.collect.FluentIterable;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSet;
import com.google.common.collect.ImmutableSortedMap;
import java.util.Iterator;
import java.util.Map;
import java.util.Objects;
import java.util.function.Function;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import org.jetbrains.annotations.Nullable;

/**
 * List of fields to add to a generated {@code BuildConfig.java} file. Each field knows its Java
 * type, variable name, and value.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class BuildConfigFields implements Iterable<Field> {

  /** An individual field in a {@link BuildConfigFields}. */
  public static class Field {

    private final String type;
    private final String name;
    private final String value;

    Field(String type, String name, String value) {
      this.type = type;
      this.name = name;
      this.value = value;
    }

    public String getType() {
      return type;
    }

    public String getName() {
      return name;
    }

    public String getValue() {
      return value;
    }

    public static Field of(String type, String name, String value) {
      return new Field(type, name, value);
    }

    /**
     * @return a string that could be passed to {@link
     *     BuildConfigFields#fromFieldDeclarations(Iterable)} such that it could be parsed to return
     *     a {@link Field} equal to this object.
     */
    @Override
    public String toString() {
      return String.format("%s %s = %s", getType(), getName(), getValue());
    }

    @Override
    public int hashCode() {
      return Objects.hash(type, name, value);
    }

    @Override
    public boolean equals(@Nullable Object obj) {
      if (this == obj) {
        return true;
      }
      if (obj == null || getClass() != obj.getClass()) {
        return false;
      }
      Field other = (Field) obj;
      return Objects.equals(type, other.type)
          && Objects.equals(name, other.name)
          && Objects.equals(value, other.value);
    }
  }

  private static final BuildConfigFields INSTANCE = new BuildConfigFields(ImmutableMap.of());

  private static final Pattern VARIABLE_DEFINITION_PATTERN =
      Pattern.compile(
          "(?<type>[a-zA-Z_$][a-zA-Z0-9_.<>]+("
              + Pattern.quote("[]")
              + ")?)"
              + "\\s+"
              + "(?<name>[a-zA-Z_$][a-zA-Z0-9_$]+)"
              + "\\s*=\\s*"
              + "(?<value>.+)");

  private static final ImmutableSet<String> PRIMITIVE_NUMERIC_TYPE_NAMES =
      ImmutableSet.of("byte", "char", "double", "float", "int", "long", "short");

  private static final Function<String, Field> TRANSFORM =
      input -> {
        Matcher matcher = VARIABLE_DEFINITION_PATTERN.matcher(input);
        if (matcher.matches()) {
          return Field.of(
              Objects.requireNonNull(matcher.group("type")),
              Objects.requireNonNull(matcher.group("name")),
              Objects.requireNonNull(matcher.group("value")));
        } else {
          throw new RuntimeException(
              String.format("Not a valid BuildConfig variable declaration: %s", input));
        }
      };

  private Map<String, BuildConfigFields.Field> nameToField;

  private BuildConfigFields(Map<String, BuildConfigFields.Field> nameToField) {
    this.nameToField = nameToField;
  }

  public Map<String, BuildConfigFields.Field> getNameToField() {
    return nameToField;
  }

  public static BuildConfigFields of(Map<String, BuildConfigFields.Field> nameToField) {
    if (nameToField.isEmpty()) {
      return INSTANCE;
    }
    return new BuildConfigFields(nameToField);
  }

  public static BuildConfigFields of() {
    return INSTANCE;
  }

  public static BuildConfigFields fromFieldDeclarations(Iterable<String> declarations) {
    return fromFields(FluentIterable.from(declarations).transform(TRANSFORM::apply));
  }

  /**
   * @return a {@link BuildConfigFields} that contains the specified fields in iteration order.
   */
  public static BuildConfigFields fromFields(Iterable<Field> fields) {
    ImmutableMap<String, Field> entries = FluentIterable.from(fields).uniqueIndex(Field::getName);
    return of(entries);
  }

  /**
   * @return A new {@link BuildConfigFields} with all of the fields from this object, combined with
   *     all of the fields from the specified {@code fields}. If both objects have fields with the
   *     same name, the entry from the {@code fields} parameter wins.
   */
  public BuildConfigFields putAll(BuildConfigFields fields) {

    ImmutableMap.Builder<String, Field> nameToFieldBuilder = ImmutableMap.builder();
    nameToFieldBuilder.putAll(fields.getNameToField());
    for (Field field : this.getNameToField().values()) {
      if (!fields.getNameToField().containsKey(field.getName())) {
        nameToFieldBuilder.put(field.getName(), field);
      }
    }
    return of(nameToFieldBuilder.build());
  }

  public BuildConfigFields overrideExisting(BuildConfigFields overrides) {
    ImmutableMap.Builder<String, Field> nameToFieldBuilder = ImmutableMap.builder();
    for (Field field : this) {
      nameToFieldBuilder.put(
          field.getName(), overrides.getNameToField().getOrDefault(field.getName(), field));
    }
    return of(nameToFieldBuilder.build());
  }

  /**
   * Creates the Java code for a {@code BuildConfig.java} file in the specified {@code javaPackage}.
   *
   * @param source The build target of the rule that is responsible for generating this
   *     BuildConfig.java file.
   * @param javaPackage The Java package for the generated file.
   * @param useConstantExpressions Whether the value of each field in the generated Java code should
   *     be the literal value from the {@link Field} (i.e., a constant expression) or a
   *     non-constant-expression that is guaranteed to evaluate to the literal value.
   */
  public String generateBuildConfigDotJava(
      String source, String javaPackage, boolean useConstantExpressions) {

    StringBuilder builder = new StringBuilder();
    // By design, we drop the flavor from the BuildTarget (if present), so this debug text makes
    // more sense to users.
    builder.append(String.format("// Generated by %s. DO NOT MODIFY.\n", source));
    builder.append("package ").append(javaPackage).append(";\n");
    builder.append("public class BuildConfig {\n");
    builder.append("  private BuildConfig() {}\n");

    String prefix = "  public static final ";
    for (Field field : ImmutableSortedMap.copyOf(getNameToField()).values()) {
      String type = field.getType();
      if ("boolean".equals(type)) {
        // type is a non-numeric primitive.
        boolean isTrue = "true".equals(field.getValue());
        if (!(isTrue || "false".equals(field.getValue()))) {
          throw new RuntimeException(
              String.format("expected boolean literal but was: %s", field.getValue()));
        }
        String value;
        if (useConstantExpressions) {
          value = String.valueOf(isTrue);
        } else {
          value = "Boolean.parseBoolean(null)";
          if (isTrue) {
            value = "!" + value;
          }
        }
        builder
            .append(prefix)
            .append("boolean ")
            .append(field.getName())
            .append(" = ")
            .append(value)
            .append(";\n");
      } else {
        String typeSafeZero = PRIMITIVE_NUMERIC_TYPE_NAMES.contains(type) ? "0" : "null";
        String defaultValue = field.getValue();
        if (!useConstantExpressions) {
          defaultValue = "!Boolean.parseBoolean(null) ? " + defaultValue + " : " + typeSafeZero;
        }
        builder
            .append(prefix)
            .append(type)
            .append(" ")
            .append(field.getName())
            .append(" = ")
            .append(defaultValue)
            .append(";\n");
      }
    }

    builder.append("}\n");
    return builder.toString();
  }

  /**
   * @return iterator that enumerates the fields used to construct this {@link BuildConfigFields}.
   *     The {@link Iterator#remove()} method of the return value is not supported.
   */
  @Override
  public Iterator<Field> iterator() {
    return getNameToField().values().iterator();
  }

  @Override
  public String toString() {
    return Joiner.on(';').join(getNameToField().values());
  }
}
