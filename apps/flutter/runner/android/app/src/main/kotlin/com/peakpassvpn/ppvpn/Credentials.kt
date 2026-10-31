package com.peakpassvpn.ppvpn

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import java.security.KeyStore
import java.security.UnrecoverableKeyException
import javax.crypto.AEADBadTagException
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

// The saved sign-in's key (#473): an AES-GCM key in the Android Keystore,
// made on first use, never leaving it. The client's Rust (apps/flutter/rust
// android.rs) calls seal and open over JNI, by name, and keeps the sealed
// bytes in a file of the app's own (credentials.rs). Sealed: the 12-byte IV,
// then the ciphertext with its tag.
object Credentials {
    private const val KEYSTORE = "AndroidKeyStore"
    private const val ALIAS = "ppvpn-credentials"
    private const val TRANSFORMATION = "AES/GCM/NoPadding"
    private const val IV_BYTES = 12
    private const val TAG_BITS = 128

    @JvmStatic
    fun seal(blob: ByteArray): ByteArray {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        // The Keystore picks the IV.
        cipher.init(Cipher.ENCRYPT_MODE, key() ?: create())
        return cipher.iv + cipher.doFinal(blob)
    }

    /**
     * The blob, or null when it cannot be opened ever again: the key is
     * gone or invalidated, or the bytes are not sealed by it (the app's
     * data restored on another device). Other failures throw.
     */
    @JvmStatic
    fun open(sealed: ByteArray): ByteArray? {
        if (sealed.size <= IV_BYTES) return null
        return try {
            val key = key() ?: return null
            val cipher = Cipher.getInstance(TRANSFORMATION)
            cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(TAG_BITS, sealed, 0, IV_BYTES))
            cipher.doFinal(sealed, IV_BYTES, sealed.size - IV_BYTES)
        } catch (e: AEADBadTagException) {
            null
        } catch (e: KeyPermanentlyInvalidatedException) {
            null
        } catch (e: UnrecoverableKeyException) {
            null
        }
    }

    private fun key(): SecretKey? {
        val store = KeyStore.getInstance(KEYSTORE).apply { load(null) }
        return store.getKey(ALIAS, null) as? SecretKey
    }

    private fun create(): SecretKey {
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE)
        generator.init(
            KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build(),
        )
        return generator.generateKey()
    }
}
