package global.auros.comrade

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import global.auros.comrade.ui.theme.ComradeTheme

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            ComradeTheme {
                Surface(
                    modifier = Modifier.fillMaxSize(),
                    color = MaterialTheme.colorScheme.background,
                ) {
                    ComradeApp()
                }
            }
        }
    }
}

@Composable
fun ComradeApp() {
    val version = remember { ComradeCore.getVersion() }
    val workspaces = remember { ComradeCore.workspaces() }

    LazyColumn(
        modifier = Modifier
            .fillMaxSize()
            .padding(horizontal = 24.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(12.dp),
        contentPadding = PaddingValues(vertical = 48.dp),
    ) {
        item {
            Text(
                text = "Comrade",
                style = MaterialTheme.typography.displayMedium,
            )
            Text(
                text = "Privacy-first social client",
                style = MaterialTheme.typography.titleMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Spacer(Modifier.height(8.dp))
            Text(
                text = "core v$version",
                style = MaterialTheme.typography.labelSmall,
                fontFamily = FontFamily.Monospace,
                color = MaterialTheme.colorScheme.outline,
            )
        }

        item { HorizontalDivider(modifier = Modifier.padding(vertical = 8.dp)) }

        item {
            Text(
                text = "Workspaces",
                style = MaterialTheme.typography.titleSmall,
                color = MaterialTheme.colorScheme.primary,
                modifier = Modifier.fillMaxWidth(),
            )
        }

        items(workspaces) { ws ->
            WorkspaceCard(info = ws)
        }

        item { HorizontalDivider(modifier = Modifier.padding(vertical = 8.dp)) }

        item { KeygenSection() }
    }
}

@Composable
fun WorkspaceCard(info: ComradeCore.WorkspaceInfo) {
    OutlinedCard(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp)) {
            Text(
                text = info.key,
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.primary,
            )
            Spacer(Modifier.height(4.dp))
            Text(
                text = info.label,
                style = MaterialTheme.typography.bodyMedium,
            )
        }
    }
}

@Composable
fun KeygenSection() {
    var keypair by remember { mutableStateOf<ComradeCore.Keypair?>(null) }
    var error by remember { mutableStateOf<String?>(null) }

    Column(
        modifier = Modifier.fillMaxWidth(),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text(
            text = "Key Management",
            style = MaterialTheme.typography.titleSmall,
            color = MaterialTheme.colorScheme.primary,
        )

        Button(
            onClick = {
                runCatching { ComradeCore.generateKeypairTyped() }
                    .onSuccess { keypair = it; error = null }
                    .onFailure { error = it.message }
            },
            modifier = Modifier.fillMaxWidth(),
        ) {
            Text("Generate New Keypair")
        }

        keypair?.let { kp ->
            OutlinedCard(modifier = Modifier.fillMaxWidth()) {
                Column(modifier = Modifier.padding(12.dp)) {
                    Text("npub", style = MaterialTheme.typography.labelSmall)
                    Text(
                        kp.npub,
                        style = MaterialTheme.typography.bodySmall,
                        fontFamily = FontFamily.Monospace,
                    )
                    Spacer(Modifier.height(8.dp))
                    Text("nsec (keep secret!)", style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.error)
                    Text(
                        kp.nsec,
                        style = MaterialTheme.typography.bodySmall,
                        fontFamily = FontFamily.Monospace,
                    )
                }
            }
        }

        error?.let {
            Text(it, color = MaterialTheme.colorScheme.error,
                style = MaterialTheme.typography.bodySmall)
        }
    }
}
