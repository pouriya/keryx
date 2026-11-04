// Google's Maven through its redirector, since dl.google.com refuses this network, as the moq
// experiment in /p/test/android found.
pluginManagement {
    repositories {
        maven { url = uri("https://redirector.gvt1.com/edgedl/android/maven2") }
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        maven { url = uri("https://redirector.gvt1.com/edgedl/android/maven2") }
        mavenCentral()
    }
}

rootProject.name = "keryx"
include(":app")
