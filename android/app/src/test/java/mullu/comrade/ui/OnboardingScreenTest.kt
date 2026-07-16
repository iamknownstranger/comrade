package mullu.comrade.ui

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pins the digits-only passcode rule ([isValidPasscode]): the create/confirm/
 * unlock fields all show a numeric keypad (`KeyboardType.NumberPassword`), so
 * anything accepted at create time must be typeable there.
 */
class OnboardingScreenTest {

    @Test
    fun digitsOnlyIsValid() {
        assertTrue(isValidPasscode("123456"))
        assertTrue(isValidPasscode("000000"))
    }

    @Test
    fun lettersOrSymbolsAreRejected() {
        assertFalse(isValidPasscode("abc123"))
        assertFalse(isValidPasscode("passcode"))
        assertFalse(isValidPasscode("123-456"))
    }

    @Test
    fun emptyPasscodeIsRejected() {
        assertFalse(isValidPasscode(""))
    }
}
