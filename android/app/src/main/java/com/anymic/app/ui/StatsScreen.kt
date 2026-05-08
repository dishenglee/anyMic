package com.anymic.app.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.navigation.NavController
import com.anymic.app.R
import com.anymic.app.model.AppState
import com.anymic.app.model.StreamStats
import kotlinx.coroutines.delay

private const val RTT_HISTORY_SECONDS = 30

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun StatsScreen(viewModel: MainViewModel, navController: NavController) {
    val state by viewModel.state.collectAsState()

    // RTT history for the mini chart (30-point ring buffer)
    val rttHistory = remember { mutableStateListOf<Int>() }

    // Collect RTT samples every second while streaming
    LaunchedEffect(Unit) {
        while (true) {
            delay(1_000)
            val s = viewModel.state.value
            if (s is AppState.Streaming) {
                if (rttHistory.size >= RTT_HISTORY_SECONDS) rttHistory.removeAt(0)
                rttHistory.add(s.stats.rttMs)
            }
        }
    }

    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        topBar = {
            TopAppBar(
                title = {
                    Text(
                        text       = stringResource(R.string.screen_stats),
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
                    containerColor             = MaterialTheme.colorScheme.surface,
                    titleContentColor          = MaterialTheme.colorScheme.onSurface,
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
            when (val s = state) {
                is AppState.Streaming -> StreamingStats(s.stats, rttHistory)
                else -> NotStreamingPlaceholder()
            }
        }
    }
}

@Composable
private fun StreamingStats(stats: StreamStats, rttHistory: List<Int>) {
    Column(
        modifier            = Modifier
            .fillMaxSize()
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        StatRow(R.string.stat_packets_sent,    stats.packetsSent.toString())
        StatRow(R.string.stat_bytes_sent,      formatBytes(stats.bytesSent))
        StatRow(R.string.stat_rtt,             "${stats.rttMs} ms")
        StatRow(R.string.stat_dropped_frames,  stats.droppedFrames.toString())
        StatRow(R.string.stat_source,          stats.source)
        StatRow(R.string.stat_session_id,      stats.sessionId)

        Spacer(Modifier.height(8.dp))

        // RTT line chart
        Card(
            colors   = CardDefaults.cardColors(
                containerColor = MaterialTheme.colorScheme.surfaceVariant,
            ),
            modifier = Modifier.fillMaxWidth(),
        ) {
            Column(modifier = Modifier.padding(12.dp)) {
                Text(
                    text       = stringResource(R.string.chart_rtt_title),
                    fontSize   = 13.sp,
                    fontWeight = FontWeight.SemiBold,
                    color      = MaterialTheme.colorScheme.onSurface,
                )
                Spacer(Modifier.height(8.dp))
                RttChart(
                    rttHistory = rttHistory,
                    lineColor  = MaterialTheme.colorScheme.primary,
                    modifier   = Modifier
                        .fillMaxWidth()
                        .height(100.dp),
                )
                Text(
                    text      = stringResource(R.string.chart_rtt_subtitle, RTT_HISTORY_SECONDS),
                    fontSize  = 10.sp,
                    color     = MaterialTheme.colorScheme.secondary,
                    textAlign = TextAlign.End,
                    modifier  = Modifier.fillMaxWidth(),
                )
            }
        }
    }
}

@Composable
private fun StatRow(labelRes: Int, value: String) {
    Card(
        colors   = CardDefaults.cardColors(
            containerColor = MaterialTheme.colorScheme.surfaceVariant,
        ),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Row(
            modifier              = Modifier
                .fillMaxWidth()
                .padding(horizontal = 16.dp, vertical = 10.dp),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment     = Alignment.CenterVertically,
        ) {
            Text(
                text     = stringResource(labelRes),
                fontSize = 13.sp,
                color    = MaterialTheme.colorScheme.secondary,
            )
            Text(
                text       = value,
                fontSize   = 13.sp,
                fontFamily = FontFamily.Monospace,
                fontWeight = FontWeight.Medium,
                color      = MaterialTheme.colorScheme.onSurface,
            )
        }
    }
}

@Composable
private fun RttChart(
    rttHistory: List<Int>,
    lineColor:  Color,
    modifier:   Modifier = Modifier,
) {
    Canvas(modifier = modifier) {
        if (rttHistory.size < 2) return@Canvas

        val maxRtt  = rttHistory.max().coerceAtLeast(50)
        val w       = size.width
        val h       = size.height
        val step    = w / (RTT_HISTORY_SECONDS - 1).toFloat()

        val path = Path()
        rttHistory.forEachIndexed { i, rtt ->
            val x = i * step
            val y = h - (rtt.toFloat() / maxRtt * h)
            if (i == 0) path.moveTo(x, y) else path.lineTo(x, y)
        }

        // Fill the missing portion with zero line
        drawPath(path, color = lineColor, style = Stroke(width = 2.dp.toPx()))

        // Zero baseline
        drawLine(
            color       = lineColor.copy(alpha = 0.2f),
            start       = Offset(0f, h),
            end         = Offset(w, h),
            strokeWidth = 1.dp.toPx(),
        )
    }
}

@Composable
private fun NotStreamingPlaceholder() {
    Box(
        modifier         = Modifier.fillMaxSize(),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text      = stringResource(R.string.stats_not_streaming),
            color     = MaterialTheme.colorScheme.secondary,
            textAlign = TextAlign.Center,
        )
    }
}

private fun formatBytes(bytes: Long): String {
    return when {
        bytes >= 1_048_576 -> "%.1f MB".format(bytes / 1_048_576.0)
        bytes >= 1_024     -> "%.1f KB".format(bytes / 1_024.0)
        else               -> "$bytes B"
    }
}
