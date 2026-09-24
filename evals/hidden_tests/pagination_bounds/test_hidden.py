from paginate import slice_page


def test_single_item_pages_and_out_of_range():
    items = ["a", "b", "c"]
    assert slice_page(items, page=1, page_size=1) == (["a"], True)
    assert slice_page(items, page=3, page_size=1) == (["c"], False)
    assert slice_page(items, page=4, page_size=1) == ([], False)
