import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    // The Flutter Gradle Plugin must be applied after the Android and Kotlin Gradle plugins.
    id("dev.flutter.flutter-gradle-plugin")
}

val keystorePropertiesFile = rootProject.file("keystore.properties")
val keystoreProperties = Properties().apply {
    if (keystorePropertiesFile.exists()) {
        keystorePropertiesFile.inputStream().use { load(it) }
    }
}
val repoRoot = rootProject.projectDir.resolve("../..").canonicalFile
val androidRuntimeScript = repoRoot.resolve("scripts/android-librime-runtime.sh")
val runtimeAssetsDir = layout.buildDirectory.dir("generated/keytao/assets")

android {
    namespace = "ink.rea.keytao_app"
    compileSdk = 36
    ndkVersion = "27.0.12077973"

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    defaultConfig {
        applicationId = "ink.rea.keytao_app"
        minSdk = 24
        targetSdk = 36
        // Uses the version code from pubspec.yaml. When using split APKs, 1000 * ABI_VERSION
        // is added automatically by Flutter. (https://developer.android.com/studio/build/configure-apk-splits#configure-APK-versions)
        // You can force using the value of versionCode by specifying the `-P force-version-code-ignoring-abi=true`
        // flag during build.
        versionCode = flutter.versionCode
        versionName = flutter.versionName
    }

    signingConfigs {
        if (keystorePropertiesFile.exists()) {
            create("release") {
                keyAlias = keystoreProperties.getProperty("keyAlias")
                keyPassword = keystoreProperties.getProperty("password")
                storeFile = keystoreProperties.getProperty("storeFile")?.let { file(it) }
                storePassword = keystoreProperties.getProperty("password")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
            signingConfig = signingConfigs.getByName(
                if (keystorePropertiesFile.exists()) "release" else "debug",
            )
        }
    }

    buildFeatures {
        buildConfig = true
    }

    sourceSets.getByName("main").assets.srcDirs(
        runtimeAssetsDir.get().asFile,
        repoRoot.resolve("resources"),
    )
}

kotlin {
    compilerOptions {
        jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17
    }
}

flutter {
    source = "../.."
}

dependencies {
    implementation("androidx.core:core-ktx:1.15.0")
    implementation("androidx.customview:customview:1.1.0")
    implementation("androidx.documentfile:documentfile:1.0.0")
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.json:json:20240303")
}

tasks.register<Exec>("syncKeytaoAndroidAssets") {
    group = "keytao"
    description = "Sync imported Android rime-data assets."
    commandLine(
        androidRuntimeScript.absolutePath,
        "sync",
        "--all",
        "--assets-only",
        "--android-app-dir",
        projectDir.absolutePath,
        "--assets-dir",
        runtimeAssetsDir.get().asFile.absolutePath,
    )
}

tasks.named("preBuild") {
    dependsOn("syncKeytaoAndroidAssets")
}
