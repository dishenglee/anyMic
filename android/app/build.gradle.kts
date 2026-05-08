plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
    id("com.google.protobuf") version "0.9.4"
}

android {
    namespace = "com.anymic.app"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.anymic.app"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"

        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro"
            )
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_11
        targetCompatibility = JavaVersion.VERSION_11
    }

    kotlinOptions {
        jvmTarget = "11"
    }

    buildFeatures {
        compose = true
    }

}

dependencies {
    implementation(project(":opus-jni"))
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.lifecycle.runtime.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.ui)
    implementation(libs.androidx.ui.graphics)
    implementation(libs.androidx.ui.tooling.preview)
    implementation(libs.androidx.material3)

    implementation("com.google.protobuf:protobuf-kotlin-lite:4.28.2")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")

    // T12: Navigation + ViewModel Compose integration
    implementation("androidx.navigation:navigation-compose:2.8.4")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.8.7")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.8.7")
    // Material Icons Extended (Mic, Router, Search…)
    implementation("androidx.compose.material:material-icons-extended")

    testImplementation(libs.junit)
    androidTestImplementation(libs.androidx.junit)
    androidTestImplementation(libs.androidx.espresso.core)
    androidTestImplementation(platform(libs.androidx.compose.bom))
    androidTestImplementation(libs.androidx.ui.test.junit4)
    androidTestImplementation("androidx.test:runner:1.6.2")
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
    androidTestImplementation("androidx.test:rules:1.6.1")  // for GrantPermissionRule
    androidTestImplementation(project(":opus-jni"))         // for OpusEncoder in test
    debugImplementation(libs.androidx.ui.tooling)
    debugImplementation(libs.androidx.ui.test.manifest)
}

protobuf {
    protoc { artifact = "com.google.protobuf:protoc:4.28.2" }
    generateProtoTasks {
        all().forEach { task ->
            task.builtins {
                create("java") { option("lite") }
                create("kotlin") { option("lite") }
            }
        }
    }
}

// Configure the proto source directory after evaluation, using Groovy dynamic dispatch
// via the `withGroovyBuilder` bridge (protobuf plugin adds `proto` via Groovy metaprogramming).
afterEvaluate {
    android.sourceSets.getByName("main").withGroovyBuilder {
        "proto" {
            "srcDir"("../../proto")
        }
    }
}

// ---------------------------------------------------------------------------
// On this Xiaomi device (Android 13), UiAutomation.grantRuntimePermission is
// blocked for shell uid 2000.
//
// connectedDebugAndroidTest installs the APK and runs tests in a single task body.
// We hook into it with doFirst (which runs before task body) to:
//   1. Pre-install both APKs via adb install -r (idempotent; skips if unchanged).
//   2. Grant RECORD_AUDIO via adb shell su.
// When AGP then tries to install the same APKs, it finds them already present
// (matching certificate + versionCode) and skips reinstall, preserving the grant.
//
// Note: the APK paths are resolved lazily so configuration cache is happy.
// ---------------------------------------------------------------------------
abstract class GrantAudioPermissionTask @Inject constructor(
    private val execOperations: ExecOperations
) : DefaultTask() {

    @get:Input
    abstract val appPackage: Property<String>

    @get:InputFile
    abstract val appApk: RegularFileProperty

    @get:InputFile
    abstract val testApk: RegularFileProperty

    @TaskAction
    fun grant() {
        // Install app APK.
        execOperations.exec {
            commandLine("adb", "install", "-r", appApk.get().asFile.absolutePath)
            isIgnoreExitValue = true
        }
        // Install test APK.
        execOperations.exec {
            commandLine("adb", "install", "-r", testApk.get().asFile.absolutePath)
            isIgnoreExitValue = true
        }
        // Grant RECORD_AUDIO via root.
        execOperations.exec {
            commandLine(
                "adb", "shell", "su", "-c",
                "pm grant ${appPackage.get()} android.permission.RECORD_AUDIO"
            )
            isIgnoreExitValue = true
        }
        logger.lifecycle("Pre-installed APKs and granted android.permission.RECORD_AUDIO to ${appPackage.get()}")
    }
}

tasks.register<GrantAudioPermissionTask>("grantRecordAudioPermission") {
    group = "verification"
    description = "Pre-installs APKs and grants RECORD_AUDIO via adb su (Xiaomi MIUI workaround)"
    appPackage.set("com.anymic.app")
    appApk.set(
        layout.buildDirectory.file("outputs/apk/debug/app-debug.apk")
    )
    testApk.set(
        layout.buildDirectory.file("outputs/apk/androidTest/debug/app-debug-androidTest.apk")
    )
    dependsOn("packageDebug", "packageDebugAndroidTest")
}

tasks.matching { it.name.startsWith("connectedDebugAndroidTest") }.configureEach {
    dependsOn("grantRecordAudioPermission")
}
