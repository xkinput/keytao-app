package ink.rea.keytao_app

import org.junit.Assert.*
import org.junit.Test

class KeytaoRimeDeploymentQueueTest {
    @Test fun `second request waits for the rerun result`() {
        val harness = Harness()
        val first = mutableListOf<String>()
        val joined = mutableListOf<String>()
        assertTrue(harness.request(first))
        assertFalse(harness.request(joined))
        assertTrue(harness.queue.rerun)

        harness.complete("original result")
        assertEquals(listOf("original result"), first)
        assertTrue(joined.isEmpty())
        assertTrue(harness.queue.running)
        assertFalse(harness.queue.rerun)

        harness.complete("rerun result")
        assertEquals(listOf("rerun result"), joined)
        assertFalse(harness.queue.running)
        assertEquals(2, harness.runs)
    }

    @Test fun `all requests during a run coalesce into one rerun`() {
        val harness = Harness()
        harness.request(mutableListOf())
        val joined = List(20) { mutableListOf<String>() }
        joined.forEach { assertFalse(harness.request(it)) }
        assertEquals(1, harness.runs)
        assertEquals(1, harness.queue.requests.size)

        harness.complete("original result")
        assertEquals(2, harness.runs)
        assertEquals(joined.size, harness.queue.requests.size)
        assertTrue(joined.all { it.isEmpty() })
        harness.complete("rerun result")

        assertTrue(joined.all { it == listOf("rerun result") })
        assertFalse(harness.queue.running)
        assertEquals(2, harness.runs)
    }

    @Test fun `nothing queued means no rerun and the next request can start`() {
        val harness = Harness()
        val result = mutableListOf<String>()
        assertTrue(harness.request(result))
        harness.complete("success")

        assertEquals(listOf("success"), result)
        assertFalse(harness.queue.running)
        assertFalse(harness.queue.rerun)
        assertTrue(harness.queue.requests.isEmpty())
        assertEquals(1, harness.runs)
        assertTrue(harness.request(mutableListOf()))
        assertEquals(2, harness.runs)
    }

    @Test fun `failure and timeout release the batch and preserve waiting requests`() {
        for (failure in listOf("failure", "timeout")) {
            val harness = Harness()
            val first = mutableListOf<String>()
            val joined = mutableListOf<String>()
            harness.request(first)
            harness.request(joined)
            harness.complete(failure)

            assertEquals(listOf(failure), first)
            assertTrue(joined.isEmpty())
            assertTrue(harness.queue.running)
            assertFalse(harness.queue.rerun)
            harness.complete(failure)

            assertEquals(listOf(failure), joined)
            assertFalse(harness.queue.running)
            assertTrue(harness.request(mutableListOf()))
        }
    }

    @Test fun `requests arriving during a rerun wait for its own followup`() {
        val harness = Harness()
        val duringFirst = mutableListOf<String>()
        val duringRerun = mutableListOf<String>()
        harness.request(mutableListOf())
        harness.request(duringFirst)
        harness.complete("first")
        assertFalse(harness.request(duringRerun))
        harness.complete("second")

        assertEquals(listOf("second"), duringFirst)
        assertTrue(duringRerun.isEmpty())
        harness.complete("third")
        assertEquals(listOf("third"), duringRerun)
        assertFalse(harness.queue.running)
        assertEquals(3, harness.runs)
    }

    private class Harness {
        val queue = KeytaoRimeDeploymentQueue<(String) -> Unit>()
        var runs = 0

        fun request(results: MutableList<String>): Boolean {
            val starts = queue.enqueue { results.add(it) }
            if (starts) runs++
            return starts
        }

        fun complete(result: String) {
            val completed = queue.complete()
            completed.forEach { it(result) }
            if (queue.running) runs++
        }
    }
}
