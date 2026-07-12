package mullu.comrade

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * Live off-grid mesh connectivity, as a [StateFlow] any screen can collect for
 * a persistent status indicator.
 *
 * [ComradeCore.pollEvent] drains a single process-global native receiver, so
 * only one pump loop may ever consume it ([MainActivity] already owns that
 * loop for Chitthis/DMs/requests). This object holds no polling logic of its
 * own — [MainActivity] pushes updates in as `mesh_status_changed` events
 * arrive, and seeds the initial value from [ComradeCore.meshStatusTyped].
 */
object MeshStatusMonitor {
    private val _status = MutableStateFlow(ComradeCore.MeshStatus(active = false, peerCount = 0))
    val status: StateFlow<ComradeCore.MeshStatus> = _status.asStateFlow()

    fun update(status: ComradeCore.MeshStatus) {
        _status.value = status
    }
}
