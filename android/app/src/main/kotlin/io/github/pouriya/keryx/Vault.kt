// The device's state on the phone: each account's two snapshot slots and its token, as files in the
// app's private storage, encrypted with an AES key that lives in the Android Keystore and never
// leaves it. Each server and account id has its own folder, since a username can change, and the
// one left behind is kept. Nothing here logs, since what passes through holds keys and the token.

package io.github.pouriya.keryx

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.io.File
import java.net.URLEncoder
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

class Vault(
    context: Context,
) {
    private val root = File(context.filesDir, "accounts")
    private val plain = File(context.filesDir, "plain")

    private val key: SecretKey by lazy {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getEntry(ALIAS, null) as? KeyStore.SecretKeyEntry)?.secretKey
            ?: KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").run {
                init(
                    KeyGenParameterSpec
                        .Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                        .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                        .setKeySize(256)
                        .build(),
                )
                generateKey()
            }
    }

    /** The server address and the last account id: not secret, so kept as plain text. */
    fun note(name: String): String? = File(plain, name).takeIf { it.exists() }?.readText()

    fun setNote(
        name: String,
        value: String,
    ) {
        plain.mkdirs()
        write(File(plain, name), value.toByteArray())
    }

    /** The account ids that have something kept for this server. */
    fun accounts(server: String): List<String> = folder(server, null).list()?.sorted() ?: emptyList()

    /** This account's folder: the snapshot, the token and the archive, apart per server. */
    fun accountDir(
        server: String,
        account: String,
    ): File = folder(server, account)

    fun get(
        server: String,
        account: String,
        name: String,
    ): ByteArray? {
        val file = File(folder(server, account), name)
        if (!file.exists()) {
            return null
        }
        val sealed = file.readBytes()
        return try {
            val cipher = Cipher.getInstance(CIPHER)
            cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, sealed, 0, 12))
            cipher.doFinal(sealed, 12, sealed.size - 12)
        } catch (_: java.security.GeneralSecurityException) {
            null
        }
    }

    /** One write that lands whole or not at all: a sealed copy beside it, then a rename. */
    fun put(
        server: String,
        account: String,
        name: String,
        bytes: ByteArray,
    ) {
        val cipher = Cipher.getInstance(CIPHER)
        cipher.init(Cipher.ENCRYPT_MODE, key)
        val folder = folder(server, account)
        folder.mkdirs()
        write(File(folder, name), cipher.iv + cipher.doFinal(bytes))
    }

    fun remove(
        server: String,
        account: String,
        name: String,
    ) {
        File(folder(server, account), name).delete()
    }

    private fun folder(
        server: String,
        account: String?,
    ): File {
        val byServer = File(root, URLEncoder.encode(server, "UTF-8"))
        return if (account == null) byServer else File(byServer, account)
    }

    private fun write(
        file: File,
        bytes: ByteArray,
    ) {
        val next = File(file.parentFile, file.name + ".next")
        next.outputStream().use { stream ->
            stream.write(bytes)
            stream.fd.sync()
        }
        if (!next.renameTo(file)) {
            throw java.io.IOException("could not replace ${file.name}")
        }
    }

    private companion object {
        const val ALIAS = "keryx"
        const val CIPHER = "AES/GCM/NoPadding"
    }
}
