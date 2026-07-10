package mullu.comrade.voice

import android.service.voice.VoiceInteractionService

/**
 * Registers Comrade as an assist app. Once the user selects Comrade as the
 * default digital assistant (Settings → Apps → Default apps → Digital assistant
 * app), the assist gesture — long-press the power/home button — routes to
 * [ComradeInteractionSessionService] instead of Google Assistant.
 *
 * Note: this is the *gesture* entry point, not the "Hey Comrade" voice phrase.
 * The stock Pixel hotword pipeline is reserved for Google's own keyphrases;
 * the spoken wake word lives in [WakeWordService].
 */
class ComradeInteractionService : VoiceInteractionService()
