package com.anymic.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Router
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.navigation.NavController
import com.anymic.app.R
import com.anymic.app.model.AppState
import com.anymic.app.net.DiscoveredServer

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DeviceListScreen(viewModel: MainViewModel, navController: NavController) {
    val state   by viewModel.state.collectAsState()
    val servers by viewModel.servers.collectAsState()

    val isDiscovering = state is AppState.Discovering

    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        topBar = {
            TopAppBar(
                title = {
                    Text(
                        text       = stringResource(R.string.screen_devices),
                        fontWeight = FontWeight.Bold,
                    )
                },
                navigationIcon = {
                    IconButton(onClick = { navController.popBackStack() }) {
                        Icon(
                            imageVector        = Icons.AutoMirrored.Filled.ArrowBack,
                            contentDescription = stringResource(R.string.cd_back),
                        )
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor         = MaterialTheme.colorScheme.surface,
                    titleContentColor      = MaterialTheme.colorScheme.onSurface,
                    navigationIconContentColor = MaterialTheme.colorScheme.onSurface,
                ),
            )
        },
    ) { innerPadding ->

        Surface(
            modifier = Modifier
                .fillMaxSize()
                .padding(innerPadding),
            color    = MaterialTheme.colorScheme.background,
        ) {
            Column(modifier = Modifier.fillMaxSize()) {

                // Discovery progress bar
                if (isDiscovering) {
                    LinearProgressIndicator(
                        modifier = Modifier.fillMaxWidth(),
                        color    = MaterialTheme.colorScheme.primary,
                    )
                }

                if (servers.isEmpty()) {
                    EmptyDeviceState(isDiscovering)
                } else {
                    LazyColumn(
                        modifier            = Modifier
                            .fillMaxSize()
                            .padding(horizontal = 16.dp, vertical = 8.dp),
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        items(servers, key = { it.host + it.dataPort }) { server ->
                            DeviceCard(
                                server    = server,
                                onClick   = {
                                    viewModel.connect(server)
                                    navController.navigate("home") {
                                        popUpTo("home") { inclusive = false }
                                    }
                                },
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun EmptyDeviceState(isDiscovering: Boolean) {
    Box(
        modifier         = Modifier.fillMaxSize(),
        contentAlignment = Alignment.Center,
    ) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            if (isDiscovering) {
                CircularProgressIndicator(
                    color    = MaterialTheme.colorScheme.primary,
                    modifier = Modifier.size(56.dp),
                )
                Spacer(Modifier.height(16.dp))
                Text(
                    text  = stringResource(R.string.status_scanning),
                    color = MaterialTheme.colorScheme.secondary,
                )
            } else {
                Icon(
                    imageVector        = Icons.Filled.Search,
                    contentDescription = null,
                    tint               = MaterialTheme.colorScheme.outline,
                    modifier           = Modifier.size(64.dp),
                )
                Spacer(Modifier.height(8.dp))
                Text(
                    text  = stringResource(R.string.no_devices_found),
                    color = MaterialTheme.colorScheme.secondary,
                )
            }
        }
    }
}

@Composable
private fun DeviceCard(server: DiscoveredServer, onClick: () -> Unit) {
    Card(
        onClick = onClick,
        colors  = CardDefaults.cardColors(
            containerColor = MaterialTheme.colorScheme.surfaceVariant,
        ),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Row(
            modifier          = Modifier
                .fillMaxWidth()
                .padding(16.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Icon(
                imageVector        = Icons.Filled.Router,
                contentDescription = null,
                tint               = MaterialTheme.colorScheme.primary,
                modifier           = Modifier.size(36.dp),
            )
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    text       = server.name,
                    fontWeight = FontWeight.SemiBold,
                    fontSize   = 15.sp,
                    color      = MaterialTheme.colorScheme.onSurface,
                )
                Text(
                    text       = "${server.host}  data:${server.dataPort}  ctl:${server.controlPort}",
                    fontSize   = 11.sp,
                    fontFamily = FontFamily.Monospace,
                    color      = MaterialTheme.colorScheme.secondary,
                )
                val extras = listOfNotNull(
                    server.protocolVersion?.let { "v=$it" },
                    server.codec?.let { "codec=$it" },
                    server.fingerprint?.let { "fid=${it.take(8)}" },
                ).joinToString("  ")
                if (extras.isNotEmpty()) {
                    Text(
                        text       = extras,
                        fontSize   = 10.sp,
                        fontFamily = FontFamily.Monospace,
                        color      = MaterialTheme.colorScheme.outline,
                    )
                }
            }
        }
    }
}
