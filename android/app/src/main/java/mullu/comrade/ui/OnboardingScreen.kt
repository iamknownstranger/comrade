package mullu.comrade.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import mullu.comrade.ComradeCore

/**
 * First-run and unlock flow.
 *
 * New device: pick a @username + passcode → a keypair identity is created on
 * device and sealed into the encrypted vault. Returning: passcode unlock.
 * Legacy vaults (created before usernames) are asked to claim a handle after
 * unlocking.
 *
 * Identity model: the username is a *display alias*. The real identity — what
 * peers actually message — is the keypair created here (shown as `npub…`).
 * Handles are not globally unique and cannot be, without a central registry;
 * contacts therefore pin the key on first use, so a stranger reusing your
 * handle can never receive messages meant for you.
 */
@Composable
fun OnboardingScreen(
    vaultExists: Boolean,
    unlock: suspend (passcode: String) -> ComradeCore.Profile,
    claimUsername: suspend (handle: String) -> ComradeCore.Profile,
    onReady: (ComradeCore.Profile) -> Unit,
) {
    var username by remember { mutableStateOf("") }
    var passcode by remember { mutableStateOf("") }
    var confirm by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    // A legacy vault unlocked without a username falls into the claim step.
    var claimOnly by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()

    val creating = !vaultExists && !claimOnly

    fun validate(): String? = when {
        claimOnly || creating -> {
            val handle = username.trim().removePrefix("@")
            when {
                handle.length < 3 || handle.length > 24 ->
                    "Username must be 3–24 characters."
                !handle.all { it.isLetterOrDigit() || it == '_' } ->
                    "Only letters, numbers and _ are allowed."
                !claimOnly && passcode.length < 6 ->
                    "Passcode must be at least 6 characters."
                !claimOnly && !isValidPasscode(passcode) ->
                    "Passcode must contain only numbers."
                !claimOnly && passcode != confirm ->
                    "Passcodes don't match."
                else -> null
            }
        }
        else -> if (passcode.isEmpty()) "Enter your passcode." else null
    }

    fun submit() {
        val problem = validate()
        if (problem != null) {
            error = problem
            return
        }
        busy = true
        error = null
        scope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    when {
                        claimOnly -> claimUsername(username)
                        creating -> {
                            unlock(passcode)
                            claimUsername(username)
                        }
                        else -> unlock(passcode)
                    }
                }
            }.onSuccess { profile ->
                busy = false
                if (profile.username == null) {
                    // Unlocked a pre-username vault: ask for a handle now.
                    claimOnly = true
                } else {
                    onReady(profile)
                }
            }.onFailure {
                busy = false
                error = it.message ?: "Something went wrong."
            }
        }
    }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(horizontal = 28.dp, vertical = 48.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text("⬢", style = MaterialTheme.typography.displaySmall, color = MaterialTheme.colorScheme.primary)
        Text("Comrade", style = MaterialTheme.typography.headlineMedium, fontWeight = FontWeight.Bold)
        Text(
            text = when {
                claimOnly -> "Pick a username for your existing identity."
                creating -> "Private messaging without middlemen."
                else -> "Welcome back."
            },
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = TextAlign.Center,
        )
        Spacer(Modifier.height(12.dp))

        if (creating || claimOnly) {
            OutlinedTextField(
                value = username,
                onValueChange = { username = it },
                label = { Text("Username") },
                prefix = { Text("@") },
                singleLine = true,
                enabled = !busy,
                modifier = Modifier
                    .fillMaxWidth()
                    .testTag("onboarding-username"),
            )
            Text(
                text = "Your public name, so people can find you. Your real identity " +
                    "is a cryptographic key created on this device — contacts are " +
                    "always pinned to the key, so a name-alike can never impersonate you.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }

        if (!claimOnly) {
            OutlinedTextField(
                value = passcode,
                onValueChange = { passcode = it },
                label = { Text(if (creating) "Passcode" else "Your passcode") },
                singleLine = true,
                enabled = !busy,
                visualTransformation = PasswordVisualTransformation(),
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword),
                modifier = Modifier
                    .fillMaxWidth()
                    .testTag("onboarding-passcode"),
            )
            if (creating) {
                OutlinedTextField(
                    value = confirm,
                    onValueChange = { confirm = it },
                    label = { Text("Confirm passcode") },
                    singleLine = true,
                    enabled = !busy,
                    visualTransformation = PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword),
                    modifier = Modifier
                        .fillMaxWidth()
                        .testTag("onboarding-confirm"),
                )
                Text(
                    text = "The passcode encrypts everything stored on this phone. It " +
                        "never leaves the device and cannot be recovered — pick " +
                        "something strong and memorable.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }

        error?.let {
            Text(
                it,
                color = MaterialTheme.colorScheme.error,
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.testTag("onboarding-error"),
            )
        }

        Spacer(Modifier.height(8.dp))
        Button(
            onClick = { submit() },
            enabled = !busy,
            modifier = Modifier
                .fillMaxWidth()
                .height(52.dp)
                .testTag("onboarding-submit"),
        ) {
            if (busy) {
                CircularProgressIndicator(modifier = Modifier.size(18.dp), strokeWidth = 2.dp)
            } else {
                Text(
                    when {
                        claimOnly -> "Claim username"
                        creating -> "Create my identity"
                        else -> "Unlock"
                    },
                )
            }
        }
    }
}

/**
 * True when [passcode] is non-empty and made up of digits only.
 *
 * The create/confirm/unlock fields all show a numeric keypad
 * ([KeyboardType.NumberPassword]), so this is enforced here too — at create
 * time — rather than relying on the keypad alone to keep non-digit input out.
 */
fun isValidPasscode(passcode: String): Boolean =
    passcode.isNotEmpty() && passcode.all { it.isDigit() }
