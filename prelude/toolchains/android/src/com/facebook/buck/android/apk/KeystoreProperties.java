/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.apk;

import com.facebook.infer.annotation.Nullsafe;
import com.google.common.base.Strings;
import java.io.BufferedInputStream;
import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Properties;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class KeystoreProperties {

  private final Path keystore;
  private final String storepass;
  private final String keypass;
  private final String alias;

  public KeystoreProperties(Path keystore, String storepass, String keypass, String alias) {
    this.keystore = keystore;
    this.storepass = storepass;
    this.keypass = keypass;
    this.alias = alias;
  }

  public Path getKeystore() {
    return keystore;
  }

  public String getStorepass() {
    return storepass;
  }

  public String getKeypass() {
    return keypass;
  }

  public String getAlias() {
    return alias;
  }

  public static KeystoreProperties createFromPropertiesFile(
      Path pathToStore, Path pathToKeystorePropertiesFile) throws IOException {
    Properties properties = readPropertiesFile(pathToKeystorePropertiesFile);

    String keystorePassword =
        getOrThrowException(properties, "key.store.password", pathToKeystorePropertiesFile);
    String alias = getOrThrowException(properties, "key.alias", pathToKeystorePropertiesFile);
    String aliasPassword =
        getOrThrowException(properties, "key.alias.password", pathToKeystorePropertiesFile);

    return new KeystoreProperties(pathToStore, keystorePassword, aliasPassword, alias);
  }

  private static Properties readPropertiesFile(Path propertiesFile) throws IOException {
    Properties properties = new Properties();
    try (BufferedReader reader =
        new BufferedReader(
            new InputStreamReader(
                new BufferedInputStream(Files.newInputStream(propertiesFile)),
                StandardCharsets.UTF_8))) {
      properties.load(reader);
    }

    return properties;
  }

  /**
   * @return a non-null, non-empty value for the specified property
   * @throws RuntimeException if there is no value for the specified property
   */
  private static String getOrThrowException(
      Properties properties, String propertyName, Path pathToKeystorePropertiesFile) {
    String value = Strings.nullToEmpty(properties.getProperty(propertyName)).trim();
    if (value.isEmpty()) {
      throw new RuntimeException(
          String.format(
              "properties file %s did not contain a value for the property %s",
              pathToKeystorePropertiesFile, propertyName));
    } else {
      return value;
    }
  }
}
