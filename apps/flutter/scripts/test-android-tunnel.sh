#!/usr/bin/env bash
# Compile the production service with Robolectric and a non-native host.
# Run after ci-build.sh, reusing its Gradle wrapper, JDK and Android SDK.
set -euo pipefail
app=$(cd "$(dirname "$0")/.." && pwd)
work="$app/build/android-tunnel-tests"
main="$work/app/src/main"
tests="$work/app/src/test/kotlin/com/peakpassvpn/ppvpn"
mkdir -p "$main/kotlin/com/peakpassvpn/ppvpn/tunnel" "$tests" "$main/res/drawable" "$main/res/values"
cp "$app/runner/android/app/src/main/res/drawable/ic_vpn_status.xml" "$main/res/drawable/"
cp "$app/runner/android/app/src/main/res/values/ppvpn_vpn.xml" "$main/res/values/"
cp "$app/runner/android/app/src/main/kotlin/com/peakpassvpn/ppvpn/TunnelService.kt" \
  "$main/kotlin/com/peakpassvpn/ppvpn/"
cp "$app/checks/android-tunnel/TunnelHost.kt" "$main/kotlin/com/peakpassvpn/ppvpn/tunnel/"
cp "$app/checks/android-tunnel/TunnelServiceTest.kt" "$tests/"
cat > "$work/settings.gradle.kts" <<'GRADLE'
pluginManagement { repositories { google(); mavenCentral(); gradlePluginPortal() } }
dependencyResolutionManagement { repositories { google(); mavenCentral() } }
rootProject.name = "android-tunnel-tests"
include(":app")
GRADLE
cat > "$work/gradle.properties" <<'GRADLE'
android.useAndroidX=true
org.gradle.workers.max=2
org.gradle.jvmargs=-Xmx2g
GRADLE
cat > "$work/app/build.gradle.kts" <<'GRADLE'
plugins { id("com.android.application") version "9.1.0" }
android {
    namespace = "com.peakpassvpn.ppvpn"
    compileSdk = 36
    defaultConfig { applicationId = "com.peakpassvpn.ppvpn"; minSdk = 26; targetSdk = 36 }
    testOptions {
        unitTests.isIncludeAndroidResources = true
        unitTests.all {
            it.maxHeapSize = "1g"
            it.systemProperty("robolectric.dependency.repo.url", "https://repo.maven.apache.org/maven2")
            it.testLogging.events("passed", "failed", "skipped")
        }
    }
}
dependencies {
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.robolectric:robolectric:4.17")
}
GRADLE
cat > "$main/AndroidManifest.xml" <<'MANIFEST'
<manifest xmlns:android="http://schemas.android.com/apk/res/android">
    <uses-permission android:name="android.permission.FOREGROUND_SERVICE"/>
    <uses-permission android:name="android.permission.FOREGROUND_SERVICE_SYSTEM_EXEMPTED"/>
    <uses-permission android:name="android.permission.POST_NOTIFICATIONS"/>
    <application android:theme="@android:style/Theme.Material.Light.NoActionBar">
        <service android:name=".TunnelService" android:exported="false" android:foregroundServiceType="systemExempted" android:permission="android.permission.BIND_VPN_SERVICE"/>
    </application>
</manifest>
MANIFEST
cd "$work"
"$app/android/gradlew" --no-daemon :app:testDebugUnitTest
