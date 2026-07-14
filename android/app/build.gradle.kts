import org.jetbrains.kotlin.gradle.tasks.KotlinCompile

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// ── uniffi-generated Kotlin bindings ──────────────────────────────────────────
//
// crates/comrade_jni has no hand-written JNI glue any more (no `external fun`
// declarations to keep in sync, no `System.loadLibrary` call) — its whole
// Kotlin-facing surface is generated straight from the compiled cdylib's own
// embedded uniffi metadata ("library mode": no .udl file to drift out of sync
// as the Vault/Sabha/Identity data model grows).
//
// The generated bindings are identical across build variant and target ABI
// (the metadata they're read from is architecture-independent), so this
// builds comrade_jni once for the *host* — deliberately not the `cargo ndk`
// cross-compiled build that produces the arm64-v8a/x86_64 jniLibs/*.so CI
// bundles into the APK (see android-apk.yml) — purely to extract that
// metadata, and generates into one shared directory added to `main`.
val uniffiOutDir = layout.buildDirectory.dir("generated/source/uniffi/kotlin")
val cargoWorkspaceRoot = rootProject.projectDir.resolve("..")
val hostCdylibName = when {
    org.gradle.internal.os.OperatingSystem.current().isMacOsX -> "libcomrade_jni.dylib"
    org.gradle.internal.os.OperatingSystem.current().isWindows -> "comrade_jni.dll"
    else -> "libcomrade_jni.so"
}
val hostCdylibPath = cargoWorkspaceRoot.resolve("target/debug/$hostCdylibName")

val cargoBuildHostCdylib = tasks.register<Exec>("cargoBuildHostCdylib") {
    description = "Builds comrade_jni for the host — only to read its uniffi interface metadata"
    workingDir = cargoWorkspaceRoot
    commandLine("cargo", "build", "-p", "comrade_jni")
    outputs.file(hostCdylibPath)
    outputs.upToDateWhen { hostCdylibPath.exists() }
}

val generateUniffiBindings = tasks.register<Exec>("generateUniffiBindings") {
    description = "Generates Kotlin bindings from comrade_jni's uniffi interface"
    dependsOn(cargoBuildHostCdylib)
    workingDir = cargoWorkspaceRoot
    val outDir = uniffiOutDir.get().asFile
    doFirst { outDir.deleteRecursively() }
    commandLine(
        "cargo", "run", "-p", "comrade_uniffi_bindgen", "--",
        "generate",
        "--library", hostCdylibPath.absolutePath,
        "--language", "kotlin",
        "--out-dir", outDir.absolutePath,
    )
    inputs.file(hostCdylibPath)
    outputs.dir(uniffiOutDir)
}

tasks.withType<KotlinCompile>().configureEach {
    dependsOn(generateUniffiBindings)
}

android {
    namespace = "mullu.comrade"
    compileSdk = 34

    defaultConfig {
        applicationId = "mullu.comrade"
        minSdk = 26
        targetSdk = 34
        versionCode = project.findProperty("versionCode")?.toString()?.toInt() ?: 2
        versionName = project.findProperty("versionName")?.toString() ?: "0.1.0"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    signingConfigs {
        // Release signing via environment variables — set these in CI secrets or local keystore.
        // Create the release signingConfig only when the alias is set *and* the
        // keystore file actually exists on disk. CI passes unset secrets as "",
        // so guard on blank (not just null); requiring the file too means an
        // alias-without-keystore misconfiguration falls back to the debug key
        // instead of failing the build against a missing keystore.
        val signingAlias = System.getenv("SIGNING_KEY_ALIAS")
        val keystore = rootProject.file(System.getenv("SIGNING_STORE_FILE") ?: "keystore.jks")
        if (!signingAlias.isNullOrBlank() && keystore.exists()) {
            create("release") {
                keyAlias = signingAlias
                keyPassword = System.getenv("SIGNING_KEY_PASSWORD")
                storePassword = System.getenv("SIGNING_STORE_PASSWORD")
                storeFile = keystore
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.findByName("release")
                ?: signingConfigs.getByName("debug")
        }
    }

    buildFeatures {
        compose = true
    }

    composeOptions {
        // Must match Kotlin 1.9.22 — see https://developer.android.com/jetpack/androidx/releases/compose-kotlin
        kotlinCompilerExtensionVersion = "1.5.8"
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    packaging {
        jniLibs {
            // Store .so files uncompressed in the APK so the linker mmaps them
            // straight from the archive instead of extracting a copy at install
            // time — faster cold start for the multi-MB Rust core and a smaller
            // on-device footprint. (minSdk 26 comfortably supports this.)
            useLegacyPackaging = false
        }
        resources {
            excludes += "/META-INF/{AL2.0,LGPL2.1}"
        }
    }

    sourceSets {
        getByName("main") {
            kotlin.srcDir(uniffiOutDir)
        }
    }
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2024.02.00")
    implementation(composeBom)

    implementation("androidx.core:core-ktx:1.12.0")
    implementation("androidx.activity:activity-compose:1.8.2")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.material3:material3")
    // Bottom-navigation glyphs (Home/Lock). material3 already exposes this
    // transitively; declared explicitly because MainActivity imports it. The
    // extended icon pack is deliberately NOT used — the one missing glyph
    // (Mic) is inlined as a custom ImageVector instead.
    implementation("androidx.compose.material:material-icons-core")

    // Offline "Hey Comrade" wake word + speech recognition (Apache-2.0, no cloud)
    implementation("com.alphacephei:vosk-android:0.3.47")

    // Runtime support for the uniffi-generated bindings (see the codegen setup
    // above): JNA is how the generated Kotlin calls into libcomrade_jni.so,
    // and kotlinx-coroutines-core backs its `suspend fun`s. Coroutines was
    // already present transitively via Compose; declared explicitly now that
    // this crate's own code imports it directly (ComradeCore's event-listener
    // registration).
    implementation("net.java.dev.jna:jna:5.17.0@aar")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.8.1")

    // WebRTC — voice/video call media (mic/camera capture + PeerConnection).
    //
    // NOT the `org.webrtc:google-webrtc` prebuilt named in the original design
    // notes: that artifact was only ever published to JCenter, which shut down
    // in 2021 and was never mirrored to Maven Central or Google's Maven — it no
    // longer resolves from the repositories this project uses (google() +
    // mavenCentral(), see settings.gradle.kts, with FAIL_ON_PROJECT_REPOS), so
    // depending on it would break the build outright. `io.github.webrtc-sdk` is
    // the actively-maintained, Maven-Central-published successor used across the
    // ecosystem (LiveKit/Stream); crucially it keeps the **same `org.webrtc.*`
    // package namespace**, so every `import org.webrtc.…` compiles unchanged.
    // The AAR bundles native libs for arm64-v8a/armeabi-v7a/x86/x86_64, so it
    // runs on real handsets and the x86_64 emulator lanes alike.
    implementation("io.github.webrtc-sdk:android:125.6422.07")

    debugImplementation("androidx.compose.ui:ui-tooling")

    testImplementation("junit:junit:4.13.2")
    androidTestImplementation(composeBom)
    androidTestImplementation("androidx.test.ext:junit:1.1.5")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.1")
    // Compose semantics assertions for the on-device startup test
    // (MainActivityUiTest) — the workspace list now loads asynchronously.
    androidTestImplementation("androidx.compose.ui:ui-test-junit4")
    // ActivityScenario (DeviceSmokeTest) + the AndroidJUnitRunner declared in
    // defaultConfig — neither is guaranteed transitively by ext:junit/espresso.
    androidTestImplementation("androidx.test:core:1.5.0")
    androidTestImplementation("androidx.test:runner:1.5.2")
    // GrantPermissionRule — pre-grant POST_NOTIFICATIONS so the app's first-run
    // notification prompt never pops a system dialog over the UI mid-test.
    androidTestImplementation("androidx.test:rules:1.5.0")
}
