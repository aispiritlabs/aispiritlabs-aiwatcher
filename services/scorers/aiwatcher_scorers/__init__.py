"""aiwatcher's scorer service: a framework's metrics behind one contract.

``contract`` is the wire, ``adapter`` is what a framework has to be to stand
behind it, ``adapters`` holds the two this ships — DeepEval and Opik — and
``service`` is the two routes. A new framework is one module in ``adapters``
and one line in ``adapters.KNOWN``; nothing in aiwatcher changes.
"""
