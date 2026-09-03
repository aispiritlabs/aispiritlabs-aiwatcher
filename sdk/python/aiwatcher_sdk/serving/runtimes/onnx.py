"""``Runtime::Onnx`` — a serialized graph read by one interpreter.

The second profile, and the one plan.md sequenced first among the real ones:
a fixed operator set, no packaged code, and the loader most deployments
actually want. What it adds over :mod:`~aiwatcher_sdk.serving.runtimes.weights`
is not a bigger model — it is a graph that **declares its own shapes**, and
that changes what a package is for.

Everywhere else in this system a declaration is the source: a workflow's
topology is what the producer said, a licence is what a human recorded, a
label schema is what the project wrote down. A package's ``inputs`` and
``outputs`` are the exception, because the artifact they describe carries the
same facts and can be asked. So this loader does not *trust* the declaration
and does not *ignore* it either — it **cross-checks**, and a disagreement is a
refusal naming both sides:

    the package declares features float32[?, 8] and the graph declares
    features float32[batch, 16]. These do not describe the same model

That refusal is the whole reason to write this loader before a signed reader
or a canary. A package whose declared shape is wrong is not a broken package —
it is a package that describes a *different model*, which means the version's
scores, its dataset lineage and its label order belong to something else. Every
other check in this profile compares bytes against a digest somebody wrote
down; this one compares two independent descriptions of the same thing, and it
is the only place in the chain where the model itself gets a vote.

What the profile learned about the two free-text fields plan.md asked about:

``entry_point``   enough to act on, and only because it is read as *a name in
                  this package* — an artifact's name or the last segment of its
                  URI. See :func:`~aiwatcher_sdk.serving.loader.pick_artifact`.
                  A value naming neither is a refusal, which is the honest
                  answer: an ONNX package holds a graph and often a label file,
                  and picking between them by convention is how a server loads
                  the labels as the model.
``preprocessing`` **not** enough to act on, and it should not become so.
                  ``resize:512`` and ``edge-grid:8x8`` are what the trainer
                  did, in its own words, and this loader reports them on
                  ``/v1/model`` without applying any of them. A package that
                  shipped preprocessing *code* would be a package that runs
                  code in whatever opens it, which is what
                  ``executes_packaged_code`` exists to keep visible. The caller
                  is the side that already has the raw input, so the caller is
                  the side that must have done it.

Needs ``onnxruntime`` and ``numpy``, imported lazily::

    pip install 'aiwatcher-sdk[onnx]'
"""

from __future__ import annotations

from collections.abc import Callable, Mapping, Sequence
from typing import Any, Protocol

from aiwatcher_sdk.serving.artifact import ArtifactReader, LoadError, read_verified
from aiwatcher_sdk.serving.loader import Predictor, pick_artifact

__all__ = ["ELEMENT_TYPES", "NodeArg", "OnnxLoader", "OnnxPredictor", "Session"]

#: ONNX element type names, as onnxruntime spells them, against the names a
#: ``TensorSpec`` uses. ``dtype`` on a package is free text — the vocabulary is
#: the runtime's, and an enum in the contract would be one framework's list
#: imposed on every other — so the translation lives with the runtime that owns
#: the vocabulary.
ELEMENT_TYPES: Mapping[str, str] = {
    "tensor(float)": "float32",
    "tensor(double)": "float64",
    "tensor(float16)": "float16",
    "tensor(bfloat16)": "bfloat16",
    "tensor(int64)": "int64",
    "tensor(int32)": "int32",
    "tensor(int16)": "int16",
    "tensor(int8)": "int8",
    "tensor(uint8)": "uint8",
    "tensor(bool)": "bool",
}

#: Element types a row of JSON numbers can be fed as without lying about what
#: arrived. A graph eating strings or 4-bit quantised input is refused rather
#: than coerced: the request surface speaks numbers.
_FEEDABLE = frozenset({"float32", "float64", "float16", "int64", "int32", "int16", "int8", "uint8"})


class NodeArg(Protocol):
    """One graph input or output, as onnxruntime describes it."""

    @property
    def name(self) -> str: ...
    @property
    def type(self) -> str: ...
    @property
    def shape(self) -> Sequence[Any]: ...


class Session(Protocol):
    """The part of ``onnxruntime.InferenceSession`` this profile uses.

    Named as a protocol so the gates below are testable without the wheel: a
    stub session exercises every cross-check, every refusal and the feed dtype,
    which is what keeps this file honest in a CI that does not install a
    hundred megabytes of runtime.
    """

    def get_inputs(self) -> Sequence[NodeArg]: ...
    def get_outputs(self) -> Sequence[NodeArg]: ...
    def get_providers(self) -> Sequence[str]: ...
    def run(self, output_names: Sequence[str] | None, feed: Mapping[str, Any]) -> Sequence[Any]: ...


#: Builds a session from the graph's bytes. Replaced in tests; in a deployment
#: it is :func:`_onnxruntime_session`, which is the only thing here that
#: imports the wheel.
SessionFactory = Callable[[bytes, int], Session]


def _onnxruntime() -> Any:
    try:
        import onnxruntime
    except ImportError as error:  # pragma: no cover - exercised by not having it
        raise LoadError(
            "the onnx runtime profile needs onnxruntime; install it with "
            "`pip install 'aiwatcher-sdk[onnx]'`"
        ) from error
    return onnxruntime


def _numpy() -> Any:
    try:
        import numpy
    except ImportError as error:  # pragma: no cover - exercised by not having it
        raise LoadError(
            "the onnx runtime profile needs numpy; install it with "
            "`pip install 'aiwatcher-sdk[onnx]'`"
        ) from error
    return numpy


def _onnxruntime_session(graph: bytes, threads: int) -> Session:
    """One session over the graph's bytes, on the CPU, with bounded threads.

    ``intra_op_num_threads`` is set rather than left to default because the
    default is "every core on the machine" — which, in a pod with a CPU limit,
    is a thread pool the scheduler will throttle the moment it is used. The
    concurrency gate in front of this bounds requests in flight; this bounds
    what one request is allowed to spend.

    The provider list is explicit and holds only the CPU. An accelerator is a
    deployment decision with a ``ResourceRequest`` behind it, and a runtime
    that silently picks one up is a runtime whose latency changes when somebody
    reschedules the pod.
    """
    runtime = _onnxruntime()
    options = runtime.SessionOptions()
    options.intra_op_num_threads = max(1, threads)
    options.inter_op_num_threads = 1
    session: Session = runtime.InferenceSession(
        graph, sess_options=options, providers=["CPUExecutionProvider"]
    )
    return session


class OnnxPredictor:
    """A session, the one input it eats, and the dtype it is fed as."""

    def __init__(
        self,
        session: Session,
        *,
        input_name: str,
        features: int,
        dtype: str,
        classes: Sequence[str],
        detail: Mapping[str, Any],
    ) -> None:
        self._session = session
        self._input = input_name
        self._features = features
        self._dtype = dtype
        self._classes = tuple(classes)
        self._detail = dict(detail)

    @property
    def features(self) -> int:
        return self._features

    @property
    def classes(self) -> tuple[str, ...]:
        return self._classes

    def predict(self, rows: Sequence[Sequence[float]]) -> list[list[float]]:
        numpy = _numpy()
        batch = numpy.asarray([list(row) for row in rows], dtype=self._dtype)
        outputs = self._session.run(None, {self._input: batch})
        if not outputs:
            raise LoadError("the graph returned nothing")
        first = numpy.asarray(outputs[0], dtype="float64").reshape(len(rows), -1)
        return [[float(value) for value in row] for row in first.tolist()]

    def describe(self) -> Mapping[str, Any]:
        return self._detail


class OnnxLoader:
    """Verify the graph's bytes, open it, and check what it says about itself."""

    def __init__(self, *, threads: int = 1, session_factory: SessionFactory | None = None) -> None:
        self._threads = threads
        self._factory = session_factory or _onnxruntime_session

    @property
    def runtime(self) -> str:
        return "onnx"

    @property
    def executes_packaged_code(self) -> bool:
        # A graph, not a program: a fixed operator set interpreted by the
        # runtime, with no entry point of the package's own choosing. This is
        # the property that makes ONNX the right second profile.
        return False

    def load(
        self,
        package: Mapping[str, Any],
        reader: ArtifactReader,
        *,
        version: str,
    ) -> Predictor:
        artifact = pick_artifact(package, prefer=("model", "graph", "weights"))
        _check_runtime_version(package)
        graph = read_verified(reader, artifact, version=version)

        try:
            session = self._factory(graph, self._threads)
        except LoadError:
            raise
        except Exception as error:
            raise LoadError(
                f"onnxruntime would not open {artifact.get('uri', '?')}: {error}"
            ) from error

        graph_inputs = list(session.get_inputs())
        graph_outputs = list(session.get_outputs())
        _check_one_input(graph_inputs, str(artifact.get("uri", "")))
        declared_inputs: Sequence[Mapping[str, Any]] = package.get("inputs") or []
        declared_outputs: Sequence[Mapping[str, Any]] = package.get("outputs") or []
        _cross_check(declared_inputs, graph_inputs, "input")
        _cross_check(declared_outputs, graph_outputs, "output")

        argument = graph_inputs[0]
        dtype = _element_type(argument)
        if dtype not in _FEEDABLE:
            raise LoadError(
                f"the graph eats {argument.type} and this request surface speaks rows of JSON "
                "numbers. A runtime for that input is a different profile"
            )
        features = _row_width(argument)
        classes = _classes(declared_outputs, graph_outputs)

        return OnnxPredictor(
            session,
            input_name=argument.name,
            features=features,
            dtype=dtype,
            classes=classes,
            detail={
                "entry_point": str(artifact.get("name", "")) or str(artifact.get("uri", "")),
                "declared_runtime_version": str(package.get("runtime_version", "")),
                "onnxruntime": _installed_version(),
                "providers": list(session.get_providers()),
                "threads": max(1, self._threads),
                "graph_inputs": [_render(argument) for argument in graph_inputs],
                "graph_outputs": [_render(argument) for argument in graph_outputs],
                # Reported, never applied. What the caller must already have
                # done to the input it is about to send.
                "preprocessing": [str(step) for step in package.get("preprocessing") or []],
            },
        )


def _installed_version() -> str:
    try:
        return str(_onnxruntime().__version__)
    except LoadError:  # pragma: no cover - only when the wheel is absent
        return ""


def _check_runtime_version(package: Mapping[str, Any]) -> None:
    """The build that wrote the graph, against the build that is about to read it.

    ``runtime_version`` exists so that "this graph needs an opset you do not
    have" is a deployment decision rather than a crash loop, and acting on it
    means comparing *before* the load. Older-than-declared is a refusal naming
    both; newer is fine, because ONNX runtimes read the opsets they predate.

    An unparsable value is not a refusal. It is free text by contract, a
    deployment may well write ``1.17.3-vendor``, and a loader that rejected
    what it could not parse would be enforcing a format the contract does not
    have.
    """
    declared = _version_tuple(str(package.get("runtime_version", "")))
    if declared is None:
        return
    installed = _version_tuple(_installed_version())
    if installed is None or installed >= declared:
        return
    raise LoadError(
        f"the package was written by onnxruntime {package.get('runtime_version')} and this host "
        f"has {_installed_version()}. A graph using an opset this build does not have fails at "
        "load, so this is a refusal an operator can act on rather than a crash loop"
    )


def _version_tuple(text: str) -> tuple[int, ...] | None:
    parts: list[int] = []
    for piece in text.strip().split(".")[:3]:
        digits = _leading_digits(piece)
        if not digits:
            break
        parts.append(int(digits))
    return tuple(parts) or None


def _leading_digits(piece: str) -> str:
    """``1`` from ``1rc2``. A version segment as far as it is a number."""
    out: list[str] = []
    for character in piece:
        if not character.isdigit():
            break
        out.append(character)
    return "".join(out)


def _check_one_input(graph_inputs: Sequence[NodeArg], uri: str) -> None:
    """What this request surface can actually feed, decided before it serves.

    ``instances`` is a list of rows of numbers, which is one rank-2 tensor with
    a free batch axis. Every other shape is refused *by name* here rather than
    discovered at the first request, and each refusal names the profile it
    would need — because "it loaded and every request 500s" is the outcome a
    pre-flight check exists to convert into a deployment decision.
    """
    if not graph_inputs:
        raise LoadError(f"{uri} declares no inputs, so there is nothing to feed it")
    if len(graph_inputs) > 1:
        names = ", ".join(argument.name for argument in graph_inputs)
        raise LoadError(
            f"{uri} takes {len(graph_inputs)} inputs ({names}) and this request surface sends one "
            "tensor of rows. Refused rather than fed a zero for the rest, which is a prediction "
            "about something nobody asked about"
        )
    shape = list(graph_inputs[0].shape)
    if len(shape) != 2:
        raise LoadError(
            f"{uri} eats {graph_inputs[0].name} with {len(shape)} dimensions "
            f"({_render_shape(shape)}) and this request surface sends rows of numbers, which is "
            "two. A graph eating an image tensor needs a profile whose requests carry images"
        )
    if isinstance(shape[0], int):
        raise LoadError(
            f"{uri} pins its batch axis at {shape[0]}, so a request of any other size fails at "
            "run time rather than here. Export it with a dynamic batch dimension"
        )


def _cross_check(
    declared: Sequence[Mapping[str, Any]], graph: Sequence[NodeArg], side: str
) -> None:
    """Every declared tensor against the graph's own answer for it.

    A declaration the graph does not corroborate is not a typo to be tolerated:
    it means the package describes a different model, and every number recorded
    against that version — its held-out score, its dataset, its label order —
    belongs to that other model.

    A package that declares *nothing* is not refused. Versions predate this
    field, the registry does not require it, and a graph is self-describing
    enough to serve; what an operator sees then is the graph's own shapes on
    ``/v1/model`` and no claim that anybody checked them.
    """
    if not declared:
        return
    by_name = {argument.name: argument for argument in graph}
    for spec in declared:
        name = str(spec.get("name", ""))
        argument = by_name.get(name)
        if argument is None:
            available = ", ".join(sorted(by_name)) or "none"
            raise LoadError(
                f"the package declares the {side} {name!r} and the graph has no such {side}. It "
                f"has: {available}"
            )
        found = _element_type(argument)
        wanted = str(spec.get("dtype", "")).strip()
        if wanted and found != wanted:
            raise LoadError(
                f"the package declares the {side} {name} as {wanted} and the graph declares it "
                f"{found}. These do not describe the same model"
            )
        _check_shape(spec, argument, side)


def _check_shape(spec: Mapping[str, Any], argument: NodeArg, side: str) -> None:
    declared: Sequence[Any] = spec.get("shape") or []
    if not declared:
        return
    actual = list(argument.shape)
    if len(declared) != len(actual):
        raise LoadError(
            f"the package declares the {side} {argument.name} with {len(declared)} dimensions "
            f"{_render_shape(declared)} and the graph declares {len(actual)} "
            f"{_render_shape(actual)}. These do not describe the same model"
        )
    for index, (want, have) in enumerate(zip(declared, actual, strict=True)):
        if want is None or not isinstance(have, int):
            # A free dimension on either side. A graph writes its batch axis as
            # a symbol (`batch`, `N`) and a package writes it as null; neither
            # is a claim about a number, so neither can contradict one.
            continue
        if int(want) != have:
            raise LoadError(
                f"the package declares the {side} {argument.name} dimension {index} as {want} and "
                f"the graph declares {have}. These do not describe the same model"
            )


def _classes(
    declared_outputs: Sequence[Mapping[str, Any]], graph_outputs: Sequence[NodeArg]
) -> tuple[str, ...]:
    """The label order, checked against the width of the output it names.

    ``classes`` is on the package rather than in the serving code because a
    label order that lives in a deployment silently permutes when somebody
    retrains: every metric stays finite and nothing says so. Here the graph can
    settle it. ``n`` classes over a width-``n`` output agree. Two classes over
    a width-1 output are the binary convention — the score is the probability
    of the second. Anything else is a refusal, because a classifier whose head
    is wider or narrower than its vocabulary is either mislabelled or mistrained
    and there is no way to tell which from here.
    """
    if not declared_outputs or not graph_outputs:
        return ()
    names = tuple(str(name) for name in declared_outputs[0].get("classes") or [])
    if not names:
        return ()
    width = _row_width(graph_outputs[0], default=0)
    if width in (0, len(names)):
        return names
    if width == 1 and len(names) == 2:
        return names
    raise LoadError(
        f"the package names {len(names)} classes ({', '.join(names)}) and the graph's output "
        f"{graph_outputs[0].name} is {width} wide. A head that is not as wide as its vocabulary is "
        "either mislabelled or mistrained, and nothing here can tell which"
    )


def _row_width(argument: NodeArg, *, default: int = -1) -> int:
    """How wide one row of this tensor is: its last fixed dimension.

    A rank-1 output (``[batch]``) is one score per row, which is the same thing
    a ``[batch, 1]`` head produces written differently, and both read as 1.
    """
    fixed = [dimension for dimension in argument.shape if isinstance(dimension, int)]
    if not fixed:
        if default >= 0:
            return default
        raise LoadError(
            f"the graph's {argument.name} has no fixed dimension "
            f"({_render_shape(list(argument.shape))}), so nothing here can say how wide a row is"
        )
    return fixed[-1]


def _element_type(argument: NodeArg) -> str:
    return ELEMENT_TYPES.get(argument.type, argument.type)


def _render(argument: NodeArg) -> dict[str, Any]:
    return {
        "name": argument.name,
        "dtype": _element_type(argument),
        "shape": _render_shape(list(argument.shape)),
    }


def _render_shape(shape: Sequence[Any]) -> list[Any]:
    """A shape as JSON, with a number still a number.

    A symbolic dimension is the graph's own word (``batch``, ``N``), a fixed one
    is an integer, and a package's free dimension is ``?``. Rendering all three
    as strings would make ``/v1/model`` unable to say which of ``batch`` and
    ``8`` was pinned, and that distinction is the whole subject of the
    cross-check above.
    """
    return ["?" if dimension is None else dimension for dimension in shape]
