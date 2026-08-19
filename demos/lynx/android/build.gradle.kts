import org.gradle.api.tasks.compile.JavaCompile
import org.gradle.jvm.toolchain.JavaLanguageVersion
import org.gradle.jvm.toolchain.JavaToolchainService
import org.jetbrains.kotlin.gradle.dsl.KotlinAndroidProjectExtension

plugins {
    alias(libs.plugins.android.application) apply false
    alias(libs.plugins.kotlin.android) apply false
    alias(libs.plugins.android.library) apply false
    alias(libs.plugins.kotlin.kapt) apply false
}

// Use Reactor's supported host JDK consistently across Java, Kotlin and KAPT tasks.
// Requiring a separately installed JDK 11 made a clean Reactor setup fail before compilation.
val hostJavaVersion = JavaLanguageVersion.of(17)
val forcedKotlinVersion = "1.8.10"

subprojects {
    val javaToolchains = extensions.findByType(JavaToolchainService::class.java)
    if (javaToolchains != null) {
        tasks.withType<JavaCompile>().configureEach {
            javaCompiler.set(
                javaToolchains.compilerFor {
                    languageVersion.set(hostJavaVersion)
                },
            )
        }
    }

    plugins.withId("org.jetbrains.kotlin.android") {
        extensions.configure<KotlinAndroidProjectExtension> {
            jvmToolchain(17)
        }
    }

    // Align Kotlin deps to avoid pulling newer Kotlin metadata with older AGP
    configurations.configureEach {
        resolutionStrategy.eachDependency {
            if (requested.group == "org.jetbrains.kotlin") {
                useVersion(forcedKotlinVersion)
                because("Keep Kotlin libs aligned with Kotlin plugin/AGP")
            }
        }
    }
}
