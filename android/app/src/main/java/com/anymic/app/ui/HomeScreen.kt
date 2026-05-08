package com.anymic.app.ui

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
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
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.DarkMode
import androidx.compose.material.icons.filled.LightMode
import androidx.compose.material.icons.filled.Mic
import androidx.compose.material.icons.filled.MicOff
import androidx.compose.material3.IconButton
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.scale
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.navigation.NavController
import com.anymic.app.R
import com.anymic.app.model.AppState

@Composable
fun HomeScreen(viewModel: MainViewModel, navController: NavController) {
    val state by viewModel.state.collectAsState()
    val context = LocalContext.current
    val prefs = remember(context) {
        context.getSharedPreferences("anymic_prefs", android.content.Context.MODE_PRIVATE)
    }
    val isZh = remember { java.util.Locale.getDefault().language.startsWith("zh") }
    val themeMode by viewModel.themeMode.collectAsState()
    val systemDark = isSystemInDarkTheme()
    val isDark = when (themeMode) {
        MainViewModel.ThemeMode.Dark   -> true
        MainViewModel.ThemeMode.Light  -> false
        MainViewModel.ThemeMode.System -> systemDark
    }
    var showManualDialog by rememberSaveable { mutableStateOf(false) }
    var ipInput by rememberSaveable {
        mutableStateOf(prefs.getString("last_ip", "") ?: "")
    }

    if (showManualDialog) {
        AlertDialog(
            onDismissRequest = { showManualDialog = false },
            title = { Text(if (isZh) "手动连接服务器" else "Manual Server IP") },
            text = {
                Column {
                    Text(
                        text = if (isZh)
                            "输入 Mac 的局域网 IP(在 Mac 终端运行 ifconfig 查看):"
                        else
                            "Enter the Mac's LAN IP (run `ifconfig` on the Mac to find it):",
                        fontSize = 12.sp,
                    )
                    Spacer(Modifier.height(8.dp))
                    OutlinedTextField(
                        value = ipInput,
                        onValueChange = { ipInput = it },
                        placeholder = { Text("192.168.1.10") },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        val ip = ipInput.trim()
                        if (ip.isNotEmpty()) {
                            prefs.edit().putString("last_ip", ip).apply()
                            viewModel.connectByIp(ip)
                            showManualDialog = false
                        }
                    },
                    enabled = ipInput.trim().isNotEmpty(),
                ) { Text(if (isZh) "连接" else "Connect") }
            },
            dismissButton = {
                TextButton(onClick = { showManualDialog = false }) {
                    Text(if (isZh) "取消" else "Cancel")
                }
            },
        )
    }

    // Mic icon colour driven by state
    val micColor by animateColorAsState(
        targetValue = when (state) {
            is AppState.Streaming   -> Color(0xFF4CAF50)  // green
            is AppState.Connecting  -> Color(0xFFFFC107)  // amber
            is AppState.Discovering -> Color(0xFF2196F3)  // blue
            is AppState.Error       -> Color(0xFFF44336)  // red
            else                    -> Color(0xFF607D8B)  // grey (Idle)
        },
        animationSpec = tween(400),
        label = "micColor",
    )

    val micScale by animateFloatAsState(
        targetValue = if (state is AppState.Streaming) 1.12f else 1f,
        animationSpec = tween(400),
        label = "micScale",
    )

    Surface(
        modifier = Modifier.fillMaxSize(),
        color    = MaterialTheme.colorScheme.background,
    ) {
        Column(
            modifier            = Modifier
                .fillMaxSize()
                .padding(24.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.SpaceBetween,
        ) {
            // ---- Top: theme toggle + app title ----
            Column(
                horizontalAlignment = Alignment.CenterHorizontally,
                modifier            = Modifier.fillMaxWidth(),
            ) {
                Row(
                    modifier              = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.End,
                ) {
                    IconButton(onClick = { viewModel.toggleThemeMode() }) {
                        Icon(
                            imageVector        = if (isDark) Icons.Filled.LightMode else Icons.Filled.DarkMode,
                            contentDescription = if (isZh) "切换主题" else "Toggle theme",
                            tint               = MaterialTheme.colorScheme.onSurface.copy(alpha = 0.7f),
                        )
                    }
                }
                Text(
                    text       = stringResource(R.string.app_name),
                    fontSize   = 36.sp,
                    fontWeight = FontWeight.Bold,
                    color      = MaterialTheme.colorScheme.primary,
                    letterSpacing = 2.sp,
                )
                Text(
                    text      = stringResource(R.string.app_version),
                    fontSize  = 12.sp,
                    color     = MaterialTheme.colorScheme.secondary,
                )
            }

            // ---- Centre: mic circle ----
            Box(
                contentAlignment = Alignment.Center,
                modifier         = Modifier
                    .size(180.dp)
                    .scale(micScale)
                    .clip(CircleShape)
                    .background(MaterialTheme.colorScheme.surfaceVariant)
                    .border(3.dp, micColor, CircleShape),
            ) {
                val icon = if (state is AppState.Idle || state is AppState.Error)
                    Icons.Filled.MicOff else Icons.Filled.Mic
                Icon(
                    imageVector        = icon,
                    contentDescription = null,
                    tint               = micColor,
                    modifier           = Modifier.size(96.dp),
                )
            }

            // ---- Status text ----
            Column(
                horizontalAlignment = Alignment.CenterHorizontally,
                modifier            = Modifier.fillMaxWidth(),
            ) {
                val (statusLabel, statusSub) = stateTexts(state)
                Text(
                    text      = statusLabel,
                    fontSize  = 18.sp,
                    fontWeight = FontWeight.SemiBold,
                    color     = MaterialTheme.colorScheme.onSurface,
                    textAlign = TextAlign.Center,
                )
                if (statusSub.isNotEmpty()) {
                    Spacer(Modifier.height(4.dp))
                    Text(
                        text      = statusSub,
                        fontSize  = 13.sp,
                        color     = MaterialTheme.colorScheme.secondary,
                        textAlign = TextAlign.Center,
                    )
                }

                // Error message card
                if (state is AppState.Error) {
                    Spacer(Modifier.height(8.dp))
                    Card(
                        colors = CardDefaults.cardColors(
                            containerColor = MaterialTheme.colorScheme.errorContainer,
                        ),
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        Text(
                            text     = (state as AppState.Error).message,
                            fontSize = 12.sp,
                            color    = MaterialTheme.colorScheme.onError,
                            modifier = Modifier.padding(10.dp),
                            fontFamily = FontFamily.Monospace,
                        )
                    }
                }

                // Mini stats during streaming
                if (state is AppState.Streaming) {
                    Spacer(Modifier.height(8.dp))
                    MiniStatsBar(state as AppState.Streaming, navController)
                }
            }

            // ---- Bottom: action buttons ----
            Column(
                horizontalAlignment = Alignment.CenterHorizontally,
                modifier            = Modifier.fillMaxWidth(),
            ) {
                when (state) {
                    is AppState.Idle -> {
                        Button(
                            onClick  = {
                                viewModel.discover()
                                navController.navigate("devices")
                            },
                            modifier = Modifier.fillMaxWidth(),
                        ) {
                            Text(stringResource(R.string.btn_discover), fontSize = 16.sp)
                        }
                        Spacer(Modifier.height(8.dp))
                        OutlinedButton(
                            onClick  = { showManualDialog = true },
                            modifier = Modifier.fillMaxWidth(),
                        ) {
                            Text(if (isZh) "手动输入 IP" else "Manual IP")
                        }
                    }
                    is AppState.Discovering -> {
                        Button(
                            onClick  = { navController.navigate("devices") },
                            modifier = Modifier.fillMaxWidth(),
                        ) {
                            Text(stringResource(R.string.btn_show_devices), fontSize = 16.sp)
                        }
                        Spacer(Modifier.height(8.dp))
                        OutlinedButton(
                            onClick  = { viewModel.stop() },
                            modifier = Modifier.fillMaxWidth(),
                        ) {
                            Text(stringResource(R.string.btn_stop))
                        }
                    }
                    is AppState.Connecting -> {
                        OutlinedButton(
                            onClick  = { viewModel.stop() },
                            modifier = Modifier.fillMaxWidth(),
                        ) {
                            Text(stringResource(R.string.btn_cancel))
                        }
                    }
                    is AppState.Streaming -> {
                        Button(
                            onClick  = { viewModel.stop() },
                            colors   = ButtonDefaults.buttonColors(
                                containerColor = MaterialTheme.colorScheme.error,
                                contentColor   = MaterialTheme.colorScheme.onError,
                            ),
                            modifier = Modifier.fillMaxWidth(),
                        ) {
                            Text(stringResource(R.string.btn_stop), fontSize = 16.sp)
                        }
                    }
                    is AppState.Error -> {
                        Button(
                            onClick  = {
                                viewModel.discover()
                                navController.navigate("devices")
                            },
                            modifier = Modifier.fillMaxWidth(),
                        ) {
                            Text(stringResource(R.string.btn_retry), fontSize = 16.sp)
                        }
                    }
                }
                Spacer(Modifier.height(16.dp))
                Text(
                    text       = "${if (isZh) "公众号" else "WeChat"}: 涤生AGI · anyMic v0.1.0",
                    fontSize   = 11.sp,
                    color      = MaterialTheme.colorScheme.onSurface.copy(alpha = 0.5f),
                    textAlign  = TextAlign.Center,
                    modifier   = Modifier.fillMaxWidth(),
                )
            }
        }
    }
}

@Composable
private fun MiniStatsBar(state: AppState.Streaming, navController: NavController) {
    val s = state.stats
    Card(
        modifier = Modifier
            .fillMaxWidth(),
        colors = CardDefaults.cardColors(
            containerColor = MaterialTheme.colorScheme.primaryContainer,
        ),
        onClick = { navController.navigate("stats") },
    ) {
        Row(
            modifier            = Modifier
                .fillMaxWidth()
                .padding(horizontal = 16.dp, vertical = 8.dp),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment   = Alignment.CenterVertically,
        ) {
            Text(
                text     = "pkts: ${s.packetsSent}",
                fontSize = 12.sp,
                color    = MaterialTheme.colorScheme.onPrimaryContainer,
                fontFamily = FontFamily.Monospace,
            )
            Text(
                text     = "RTT: ${s.rttMs} ms",
                fontSize = 12.sp,
                color    = MaterialTheme.colorScheme.onPrimaryContainer,
                fontFamily = FontFamily.Monospace,
            )
            Text(
                text     = "详情 >",
                fontSize = 11.sp,
                color    = MaterialTheme.colorScheme.primary,
            )
        }
    }
}

/** Returns Pair(primary status label, secondary subtitle). */
@Composable
private fun stateTexts(state: AppState): Pair<String, String> = when (state) {
    is AppState.Idle        ->
        stringResource(R.string.status_idle) to ""
    is AppState.Discovering ->
        stringResource(R.string.status_discovering) to
            stringResource(R.string.status_discovering_sub, state.servers.size)
    is AppState.Connecting  ->
        stringResource(R.string.status_connecting) to state.target.name
    is AppState.Streaming   ->
        stringResource(R.string.status_streaming) to state.target.name
    is AppState.Error       ->
        stringResource(R.string.status_error) to ""
}
