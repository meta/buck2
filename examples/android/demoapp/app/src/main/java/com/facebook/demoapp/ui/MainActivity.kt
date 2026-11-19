/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.demoapp.ui

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import com.facebook.demoapp.R
import com.facebook.demoapp.model.PostModel
import com.facebook.demoapp.ui.theme.MyApplicationTheme

class MainActivity : ComponentActivity() {

  @OptIn(ExperimentalMaterial3Api::class)
  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)

    val models = listOf(
        PostModel("Alan Turing", R.drawable.ic_launcher_background, "Some post text #1", 20),
        PostModel("Ada Lovelace", R.drawable.ic_launcher_background, "Some post text #2", 10),
        PostModel("Grace Hopper", R.drawable.ic_launcher_background, "Some post text #3", 30),
    )

    enableEdgeToEdge()
    setContent {
      MyApplicationTheme {
        Scaffold(
            modifier = Modifier.fillMaxSize(),
            topBar = {
              TopAppBar(
                  colors =
                      TopAppBarDefaults.topAppBarColors(
                          containerColor = MaterialTheme.colorScheme.primaryContainer,
                          titleContentColor = MaterialTheme.colorScheme.primary,
                      ),
                  title = { Text(text = stringResource(id = R.string.app_name)) },
              )
            },
        ) { innerPadding ->
          SocialFeed(models, modifier = Modifier.padding(innerPadding))
        }
      }
    }
  }
}
