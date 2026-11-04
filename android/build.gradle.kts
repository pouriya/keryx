// The toolchain the moq experiment in /p/test/android already built with on this machine.
plugins {
    id("com.android.application") version "8.7.3" apply false
    id("org.jetbrains.kotlin.android") version "2.0.21" apply false
    id("org.jetbrains.kotlin.plugin.compose") version "2.0.21" apply false
    id("org.jetbrains.kotlin.plugin.serialization") version "2.0.21" apply false
    id("org.jlleitschuh.gradle.ktlint") version "12.1.2" apply false
}
