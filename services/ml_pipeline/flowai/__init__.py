"""FlowAI: Python building blocks for model preparation, independent of the UI.

Encoders fit on training data, transform later batches with the same vocabulary,
and export their fitted state as JSON. The notebook service is only an adapter.
"""

from flowai.preprocessing import LabelEncoder, OneHotEncoder

__all__ = ["LabelEncoder", "OneHotEncoder"]
__version__ = "0.1.0"
