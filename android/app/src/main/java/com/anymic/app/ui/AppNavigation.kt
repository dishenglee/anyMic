package com.anymic.app.ui

import androidx.compose.runtime.Composable
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController

@Composable
fun AppNavigation(viewModel: MainViewModel) {
    val navController = rememberNavController()

    NavHost(navController = navController, startDestination = "home") {
        composable("home") {
            HomeScreen(viewModel = viewModel, navController = navController)
        }
        composable("devices") {
            DeviceListScreen(viewModel = viewModel, navController = navController)
        }
        composable("stats") {
            StatsScreen(viewModel = viewModel, navController = navController)
        }
    }
}
