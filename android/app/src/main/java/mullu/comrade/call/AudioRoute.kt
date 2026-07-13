package mullu.comrade.call

/**
 * Where in-call audio is currently playing. [CallManager.availableRoutes] lists
 * which of these are actually present on the device right now — [BLUETOOTH]
 * and [WIRED] only appear while a matching device is connected.
 */
enum class AudioRoute { EARPIECE, SPEAKER, BLUETOOTH, WIRED }
